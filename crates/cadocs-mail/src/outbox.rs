//! Where a handler leaves a message: it never waits for the relay, so neither the client nor the
//! time of an answer tells whether a message went out (ADR 0026). Messages live in memory only.
//! One the relay cannot take, or that waited too long, is logged and lost: the user asks again.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Semaphore, mpsc};

use crate::{Mail, Mailer};

/// Messages waiting for a connection. Past this, new ones are dropped.
const CAPACITY: usize = 64;
/// Connections to the relay at once.
const SENDS_AT_ONCE: usize = 4;
/// A message older than this is not worth sending: whoever asked has asked again or left.
const MAX_WAIT: Duration = Duration::from_secs(120);

struct Queued {
    mail: Mail,
    at: Instant,
}

/// Cheap to clone. The task behind it ends when the last clone is dropped.
#[derive(Clone)]
pub struct Outbox {
    queue: mpsc::Sender<Queued>,
}

impl Outbox {
    /// Spawns the task that sends, so this needs a runtime.
    pub fn start(mailer: Arc<dyn Mailer>) -> Self {
        Self::with_limits(mailer, CAPACITY, MAX_WAIT)
    }

    /// For tests: how many messages may wait, and for how long.
    pub fn with_limits(mailer: Arc<dyn Mailer>, capacity: usize, max_wait: Duration) -> Self {
        let (queue, queued) = mpsc::channel(capacity);
        tokio::spawn(send_all(mailer, queued, max_wait));
        Self { queue }
    }

    /// Leaves a message to send, without waiting. `false` when the outbox is full and the message
    /// was dropped, which the caller must not show to a client.
    pub fn queue(&self, mail: Mail) -> bool {
        match self.reserve() {
            Some(place) => {
                place.send(mail);
                true
            }
            None => false,
        }
    }

    /// A place for a message that is not written yet, without waiting, or `None` when the outbox
    /// is full. For a caller that must not change anything a message it cannot queue would tell
    /// about, such as replacing a link.
    pub fn reserve(&self) -> Option<Place> {
        let place = self.queue.clone().try_reserve_owned().ok();
        if place.is_none() {
            tracing::warn!("the outbox is full, a message was dropped");
        }
        place.map(Place)
    }
}

/// Room for one message. Dropping it gives the room back.
pub struct Place(mpsc::OwnedPermit<Queued>);

impl Place {
    pub fn send(self, mail: Mail) {
        self.0.send(Queued {
            mail,
            at: Instant::now(),
        });
    }
}

async fn send_all(mailer: Arc<dyn Mailer>, mut queued: mpsc::Receiver<Queued>, max_wait: Duration) {
    let slots = Arc::new(Semaphore::new(SENDS_AT_ONCE));
    while let Some(Queued { mail, at }) = queued.recv().await {
        // Never closed, so this only fails when the runtime ends.
        let Ok(slot) = slots.clone().acquire_owned().await else {
            return;
        };
        if at.elapsed() > max_wait {
            tracing::warn!("a message waited too long for the relay and was dropped");
            continue;
        }
        let mailer = mailer.clone();
        tokio::spawn(async move {
            if let Err(error) = mailer.send(&mail).await {
                tracing::warn!("a message was lost, {error}");
            }
            drop(slot);
        });
    }
}
