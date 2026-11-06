use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cadocs_core::permissions::SocketMode;
use tokio::sync::{OwnedSemaphorePermit, mpsc, oneshot};
use yrs::sync::Message;
use yrs::sync::protocol::{MSG_AWARENESS, MSG_SYNC};
use yrs::updates::decoder::Decode;

use crate::room::Room;
use crate::{CLOSE_FRAME_TOO_LARGE, CLOSE_GOING_AWAY, CLOSE_PROTOCOL_ERROR};

/// The close code a socket must end with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Close(pub u16);

/// What the socket layer must do next.
#[derive(Debug, Clone)]
pub enum Outbound {
    /// Send one binary frame. `y-websocket` reads a single message per frame.
    Frame(Arc<[u8]>),
    /// Close with this code. Frames still queued are dropped, as the client resyncs anyway.
    Close(u16),
}

/// One client in a room. Dropping it removes the client and its awareness states.
pub struct Connection {
    room: Arc<Room>,
    id: u64,
    frames: mpsc::Receiver<Arc<[u8]>>,
    /// Bytes in `frames`, shared with the room.
    queued_bytes: Arc<AtomicUsize>,
    close: oneshot::Receiver<u16>,
    closed: Option<u16>,
    _permit: OwnedSemaphorePermit,
}

impl Connection {
    pub(crate) fn new(
        room: Arc<Room>,
        id: u64,
        frames: mpsc::Receiver<Arc<[u8]>>,
        queued_bytes: Arc<AtomicUsize>,
        close: oneshot::Receiver<u16>,
        permit: OwnedSemaphorePermit,
    ) -> Self {
        Self {
            room,
            id,
            frames,
            queued_bytes,
            close,
            closed: None,
            _permit: permit,
        }
    }

    /// Handles a binary frame from the client. An error means the socket must close with that
    /// code, and leaves the room as it was.
    pub fn receive(&self, frame: &[u8]) -> Result<(), Close> {
        if frame.len() > self.room.max_frame_bytes() {
            return Err(Close(CLOSE_FRAME_TOO_LARGE));
        }
        if !declared_count_fits(frame) {
            return Err(Close(CLOSE_PROTOCOL_ERROR));
        }
        let message = decode_untrusted::<Message>(frame)?;
        // Encoding a reply can also reach yrs code that panics on what a crafted update left in
        // the document. The socket closes, and the room lock ignores poisoning.
        catch_unwind(AssertUnwindSafe(|| self.room.handle(self.id, message)))
            .unwrap_or(Err(Close(CLOSE_PROTOCOL_ERROR)))
    }

    /// The next thing to do on the socket. Cancel safe, so it can sit in a `select!`.
    pub async fn next(&mut self) -> Outbound {
        if let Some(code) = self.closed {
            return Outbound::Close(code);
        }
        tokio::select! {
            biased;
            code = &mut self.close => {
                let code = code.unwrap_or(CLOSE_GOING_AWAY);
                self.closed = Some(code);
                Outbound::Close(code)
            }
            frame = self.frames.recv() => match frame {
                Some(frame) => {
                    self.queued_bytes.fetch_sub(frame.len(), Ordering::Relaxed);
                    Outbound::Frame(frame)
                }
                None => {
                    self.closed = Some(CLOSE_GOING_AWAY);
                    Outbound::Close(CLOSE_GOING_AWAY)
                }
            },
        }
    }

    /// Ends this socket with `code`, as a recheck of another task would.
    pub fn close(&self, code: u16) {
        self.room.kick(self.id, code);
    }

    pub fn mode(&self) -> SocketMode {
        self.room.mode(self.id)
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.room.leave(self.id);
    }
}

/// yrs reserves a hash map for the entry count of a state vector, an awareness update or an update
/// before reading any entry, and fills its control bytes: 8 bytes can cost 1 GB of RSS. An entry
/// takes at least two bytes, so a count above the bytes left cannot be honest.
fn declared_count_fits(frame: &[u8]) -> bool {
    let mut rest = frame;
    let tag = read_var(&mut rest);
    let payload_length = if tag == Some(u64::from(MSG_SYNC)) {
        read_var(&mut rest).and_then(|_sub_type| read_var(&mut rest))
    } else if tag == Some(u64::from(MSG_AWARENESS)) {
        read_var(&mut rest)
    } else {
        return tag.is_some();
    };
    let count = payload_length.and_then(|_| read_var(&mut rest));
    count.is_some_and(|count| count <= rest.len() as u64)
}

/// Reads a lib0 variable-length unsigned integer.
fn read_var(bytes: &mut &[u8]) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let (byte, rest) = bytes.split_first()?;
        *bytes = rest;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// Decodes bytes from a client. yrs can panic on crafted input. Nothing is shared with a decoder, so catching that panic is safe, and the client
/// gets the same close as for any other malformed frame.
pub(crate) fn decode_untrusted<T: Decode>(bytes: &[u8]) -> Result<T, Close> {
    catch_unwind(|| T::decode_v1(bytes))
        .ok()
        .and_then(Result::ok)
        .ok_or(Close(CLOSE_PROTOCOL_ERROR))
}
