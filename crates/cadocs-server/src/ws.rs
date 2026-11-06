//! The page and events sockets: checks before the upgrade, then frames between a WebSocket and a
//! `Connection` or an `EventConnection`.

use std::time::Duration;

use axum::Extension;
use axum::body::Bytes;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use cadocs_collab::{
    Authorizer, CLOSE_FRAME_TOO_LARGE, CLOSE_PROTOCOL_ERROR, CLOSE_TOO_SLOW, Close, CollabError,
    Connection, EventConnection, EventOutbound, EventSeat, EventStanding, Outbound, Seat,
};
use cadocs_core::permissions::SocketOutcome;
use cadocs_store::DocStore;
use tokio::time::{Instant, MissedTickBehavior, interval_at, timeout};

use crate::AppState;
use crate::config::SocketConfig;
use crate::session::CurrentSession;

/// tungstenite defaults to 128 KiB each way, per socket, and most frames are a few hundred bytes.
/// These are starting sizes, not limits: the write buffer keeps the capacity of the largest frame
/// sent, so each socket holds about one copy of its document's SyncStep2 for its whole life.
const READ_BUFFER_BYTES: usize = 4 * 1024;
const WRITE_BUFFER_BYTES: usize = 4 * 1024;

/// An events frame is a hundred bytes or so, and a client has nothing to send but pongs.
const EVENTS_BUFFER_BYTES: usize = 1024;
const EVENTS_MAX_FRAME_BYTES: usize = 1024;

/// How long a closing socket may wait for the client's side of the close handshake.
const CLOSE_GRACE: Duration = Duration::from_secs(1);

/// Behind `api::check_origin` and `session::require`, so a request that gets here has a user.
pub async fn upgrade<S: DocStore, A: Authorizer>(
    Path(page_id): Path<String>,
    State(state): State<AppState<S, A>>,
    Extension(session): Extension<CurrentSession>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let user_id = &session.user.id;
    let mode = match state.authorizer.authorize(&page_id, user_id).await {
        Ok(Some(mode)) => mode,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            tracing::error!(page_id, %error, "cannot resolve a role");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };
    let hash = session.token_hash();
    let connecting = state.registry.connect(&page_id, mode, user_id, &hash);
    let connection = match connecting.await {
        Ok(connection) => connection,
        Err(CollabError::AtCapacity | CollabError::ShuttingDown) => {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        Err(error) => {
            tracing::error!(page_id, %error, "cannot open the document");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    // A recheck only finds sockets that have joined. This covers a change made between the check
    // above and the join: its pass may have come and gone, but a later change has a pass of its own.
    let seat = Seat {
        page_id: &page_id,
        user_id,
        session: &hash,
        mode,
    };
    match state.authorizer.recheck(&seat).await {
        Ok(SocketOutcome::Keep) => {}
        Ok(SocketOutcome::Close(code)) => connection.close(code),
        Err(error) => {
            tracing::error!(page_id, %error, "cannot resolve a role");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    }

    let max_frame_bytes = state.registry.max_frame_bytes();
    let config = state.socket;
    let open_socket = state.sockets.subscribe();
    upgrade
        .read_buffer_size(READ_BUFFER_BYTES)
        .write_buffer_size(WRITE_BUFFER_BYTES)
        .max_frame_size(max_frame_bytes)
        .max_message_size(max_frame_bytes)
        .on_upgrade(move |socket| async move {
            run(socket, connection, config).await;
            drop(open_socket);
        })
}

/// `?workspace=` holds an id, which needs no decoding.
fn workspace_param(query: Option<&str>) -> Option<&str> {
    let mut pairs = query.unwrap_or_default().split('&');
    pairs.find_map(|pair| pair.strip_prefix("workspace="))
}

/// Behind `api::check_origin` and `session::require`, as a page socket is. A workspace the user is
/// not a member of answers as one that does not exist.
pub async fn upgrade_events<S: DocStore, A: Authorizer>(
    RawQuery(query): RawQuery,
    State(state): State<AppState<S, A>>,
    Extension(session): Extension<CurrentSession>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let Some(workspace_id) = workspace_param(query.as_deref()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let user_id = &session.user.id;
    let hash = session.token_hash();
    let seat = EventSeat {
        workspace_id,
        user_id,
        session: &hash,
    };
    let spaces = match state.authorizer.recheck_events(&seat).await {
        Ok(EventStanding::Open(spaces)) => spaces,
        Ok(EventStanding::Close(_)) => return StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            tracing::error!(workspace_id, %error, "cannot resolve a member");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };
    let joined = state
        .registry
        .join_events(workspace_id, user_id, &hash, spaces);
    let Ok(connection) = joined else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };

    // As for a page socket: a recheck only finds sockets that have joined. `settle` keeps what a
    // recheck decided in the meantime.
    match state.authorizer.recheck_events(&seat).await {
        Ok(standing) => connection.settle(standing),
        Err(error) => {
            tracing::error!(workspace_id, %error, "cannot resolve a member");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    }

    let config = state.socket;
    let open_socket = state.sockets.subscribe();
    upgrade
        .read_buffer_size(EVENTS_BUFFER_BYTES)
        .write_buffer_size(EVENTS_BUFFER_BYTES)
        .max_frame_size(EVENTS_MAX_FRAME_BYTES)
        .max_message_size(EVENTS_MAX_FRAME_BYTES)
        .on_upgrade(move |socket| async move {
            run_events(socket, connection, config).await;
            drop(open_socket);
        })
}

async fn run_events(mut socket: WebSocket, mut connection: EventConnection, config: SocketConfig) {
    let code = pump_events(&mut socket, &mut connection, config).await;
    // Its place in the socket count is free now, whatever the close handshake takes.
    drop(connection);
    let _ = timeout(CLOSE_GRACE, close(&mut socket, code)).await;
}

/// `pump` for a socket that only sends: anything a client sends but a pong closes it.
async fn pump_events(
    socket: &mut WebSocket,
    connection: &mut EventConnection,
    config: SocketConfig,
) -> Option<u16> {
    let mut ping = interval_at(Instant::now() + config.ping, config.ping);
    ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut last_heard = Instant::now();

    loop {
        let outgoing = tokio::select! {
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Pong(_))) => {
                    last_heard = Instant::now();
                    continue;
                }
                Some(Ok(Message::Close(_))) | None => return None,
                Some(Ok(_)) => return Some(CLOSE_PROTOCOL_ERROR),
                Some(Err(error)) => return read_error_code(error),
            },
            outbound = connection.next() => match outbound {
                EventOutbound::Text(frame) => Message::Text(frame.into()),
                EventOutbound::Close(code) => return Some(code),
            },
            _ = ping.tick() => {
                if last_heard.elapsed() >= config.ping_timeout {
                    return Some(CLOSE_TOO_SLOW);
                }
                Message::Ping(Bytes::new())
            }
        };
        match timeout(config.ping_timeout, socket.send(outgoing)).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => return None,
            Err(_) => return Some(CLOSE_TOO_SLOW),
        }
    }
}

async fn run(mut socket: WebSocket, mut connection: Connection, config: SocketConfig) {
    let code = pump(&mut socket, &mut connection, config).await;
    // The client leaves the room now, whatever the close handshake takes.
    drop(connection);
    let _ = timeout(CLOSE_GRACE, close(&mut socket, code)).await;
}

/// Moves frames until the socket must end, and returns the close code to send, if any.
///
/// One task does everything, so a blocked send also stops reads. The send timeout bounds that,
/// and a client that stops reading only stalls its own socket.
async fn pump(
    socket: &mut WebSocket,
    connection: &mut Connection,
    config: SocketConfig,
) -> Option<u16> {
    let mut ping = interval_at(Instant::now() + config.ping, config.ping);
    ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut last_heard = Instant::now();

    loop {
        let outgoing = tokio::select! {
            incoming = socket.recv() => {
                let message = match incoming {
                    Some(Ok(message)) => message,
                    Some(Err(error)) => return read_error_code(error),
                    None => return None,
                };
                last_heard = Instant::now();
                match message {
                    Message::Binary(frame) => {
                        if let Err(Close(code)) = connection.receive(&frame) {
                            return Some(code);
                        }
                    }
                    Message::Text(_) => return Some(CLOSE_PROTOCOL_ERROR),
                    Message::Ping(_) | Message::Pong(_) => {}
                    Message::Close(_) => return None,
                }
                continue;
            }
            outbound = connection.next() => match outbound {
                Outbound::Frame(frame) => Message::Binary(Bytes::from_owner(frame)),
                Outbound::Close(code) => return Some(code),
            },
            _ = ping.tick() => {
                if last_heard.elapsed() >= config.ping_timeout {
                    return Some(CLOSE_TOO_SLOW);
                }
                Message::Ping(Bytes::new())
            }
        };
        match timeout(config.ping_timeout, socket.send(outgoing)).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => return None,
            Err(_) => return Some(CLOSE_TOO_SLOW),
        }
    }
}

/// tungstenite enforces the frame limit while reading, so a large frame is never buffered whole,
/// but it reports this as a read error and leaves the close code to us.
fn read_error_code(error: axum::Error) -> Option<u16> {
    match error.into_inner().downcast_ref::<tungstenite::Error>() {
        Some(tungstenite::Error::Capacity(_)) => Some(CLOSE_FRAME_TOO_LARGE),
        _ => None,
    }
}

/// Sends the close frame if we are the one closing, then reads until the end. That read is what
/// flushes the answer to a client's close, and closing the TCP connection with unread data would
/// reset it and could lose our frame.
async fn close(socket: &mut WebSocket, code: Option<u16>) {
    if let Some(code) = code {
        let frame = CloseFrame {
            code,
            reason: "".into(),
        };
        if socket.send(Message::Close(Some(frame))).await.is_err() {
            return;
        }
    }
    while let Some(Ok(_)) = socket.recv().await {}
}
