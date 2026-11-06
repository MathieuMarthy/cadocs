//! The page socket: checks before the upgrade, then frames between a WebSocket and a `Connection`.

use std::time::Duration;

use axum::body::Bytes;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use cadocs_collab::{
    Authorizer, CLOSE_FRAME_TOO_LARGE, CLOSE_PROTOCOL_ERROR, CLOSE_TOO_SLOW, Close, CollabError,
    Connection, Outbound,
};
use cadocs_store::DocStore;
use tokio::time::{Instant, MissedTickBehavior, interval_at, timeout};

use crate::AppState;
use crate::config::SocketConfig;

const MAX_DOC_ID_LEN: usize = 64;

/// tungstenite defaults to 128 KiB each way, per socket, and most frames are a few hundred bytes.
/// These are starting sizes, not limits: the write buffer keeps the capacity of the largest frame
/// sent, so each socket holds about one copy of its document's SyncStep2 for its whole life.
const READ_BUFFER_BYTES: usize = 4 * 1024;
const WRITE_BUFFER_BYTES: usize = 4 * 1024;

/// How long a closing socket may wait for the client's side of the close handshake.
const CLOSE_GRACE: Duration = Duration::from_secs(1);

pub async fn upgrade<S: DocStore, A: Authorizer>(
    Path(doc_id): Path<String>,
    State(state): State<AppState<S, A>>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !valid_doc_id(&doc_id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(mode) = state.authorizer.authorize(&doc_id).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let connection = match state.registry.connect(&doc_id, mode).await {
        Ok(connection) => connection,
        Err(CollabError::AtCapacity | CollabError::ShuttingDown) => {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        Err(error) => {
            tracing::error!(doc_id, %error, "cannot open the document");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

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

/// Without accounts there is no public URL to compare with. The page must come from the host it
/// connects to, which stops other websites, and that host must be a loopback name, which stops DNS
/// rebinding, where a hostile name makes both headers agree. Clients outside a browser send no
/// `Origin`. `CADOCS_PUBLIC_URL` replaces this in the MVP.
fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let host = headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok());
    let origin_host = origin
        .to_str()
        .ok()
        .and_then(|origin| origin.split_once("://"))
        .map(|(_, host)| host);
    matches!((origin_host, host), (Some(a), Some(b)) if a.eq_ignore_ascii_case(b) && is_loopback(b))
}

fn is_loopback(host: &str) -> bool {
    let name = match host.rsplit_once(':') {
        Some((name, port)) if port.bytes().all(|byte| byte.is_ascii_digit()) => name,
        _ => host,
    };
    let name = name.to_ascii_lowercase();
    matches!(name.as_str(), "localhost" | "127.0.0.1" | "[::1]")
}

fn valid_doc_id(doc_id: &str) -> bool {
    (1..=MAX_DOC_ID_LEN).contains(&doc_id.len())
        && doc_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
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
