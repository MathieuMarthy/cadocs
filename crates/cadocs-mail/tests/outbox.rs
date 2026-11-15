//! The outbox: whoever queues a message never waits for the relay.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cadocs_mail::{Mail, MailError, Mailer, Outbox, Sending};
use tokio::sync::Semaphore;

/// Keeps what it is given. Each send waits for a permit of `gate`, and `refuse` fails the
/// messages to that address.
struct Recorder {
    sent: Mutex<Vec<String>>,
    started: Mutex<Vec<String>>,
    gate: Semaphore,
    refuse: &'static str,
}

impl Recorder {
    fn new(permits: usize) -> Arc<Self> {
        Arc::new(Self {
            sent: Mutex::default(),
            started: Mutex::default(),
            gate: Semaphore::new(permits),
            refuse: "nobody@example.org",
        })
    }

    fn sent(&self) -> Vec<String> {
        self.sent.lock().unwrap().clone()
    }

    fn started(&self) -> usize {
        self.started.lock().unwrap().len()
    }
}

impl Mailer for Recorder {
    fn send<'a>(&'a self, mail: &'a Mail) -> Sending<'a> {
        Box::pin(async move {
            self.started.lock().unwrap().push(mail.to.clone());
            self.gate.acquire().await.unwrap().forget();
            if mail.to == self.refuse {
                return Err(MailError::Rejected("no such user".to_owned()));
            }
            self.sent.lock().unwrap().push(mail.to.clone());
            Ok(())
        })
    }
}

fn mail(to: &str) -> Mail {
    Mail {
        to: to.to_owned(),
        subject: "Hello".to_owned(),
        text: "Hello.\n".to_owned(),
    }
}

async fn until(what: &str, done: impl Fn() -> bool) {
    let limit = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < limit, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn a_queued_message_is_sent() {
    let recorder = Recorder::new(100);
    let outbox = Outbox::start(recorder.clone());
    assert!(outbox.queue(mail("ada@example.org")));
    until("the message goes out", || recorder.sent().len() == 1).await;
    assert_eq!(recorder.sent(), ["ada@example.org"]);
}

#[tokio::test]
async fn queueing_never_waits_for_the_relay() {
    let recorder = Recorder::new(0);
    let outbox = Outbox::start(recorder.clone());
    let before = Instant::now();
    assert!(outbox.queue(mail("ada@example.org")));
    assert!(before.elapsed() < Duration::from_millis(100));
    until("the send starts", || recorder.started() == 1).await;
    assert!(recorder.sent().is_empty());

    recorder.gate.add_permits(1);
    until("the message goes out", || recorder.sent().len() == 1).await;
}

#[tokio::test]
async fn a_refused_message_is_lost_and_the_next_ones_go_out() {
    let recorder = Recorder::new(100);
    let outbox = Outbox::start(recorder.clone());
    assert!(outbox.queue(mail("nobody@example.org")));
    assert!(outbox.queue(mail("ada@example.org")));
    until("the second message goes out", || recorder.sent().len() == 1).await;
    assert_eq!(recorder.sent(), ["ada@example.org"]);
}

#[tokio::test]
async fn a_stalled_relay_holds_a_bounded_number_of_messages() {
    let recorder = Recorder::new(0);
    let outbox = Outbox::with_limits(recorder.clone(), 3, Duration::from_secs(60));
    let fill = || {
        (0..50)
            .filter(|n| outbox.queue(mail(&format!("user{n}@example.org"))))
            .count()
    };
    // Filled again as the task takes messages out, until nothing moves any more.
    let mut taken = 0;
    for _ in 0..5 {
        taken += fill();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // Four are with the relay, one waits for its turn, three are in the queue.
    assert_eq!(recorder.started(), 4);
    assert_eq!(taken, 8);
    assert!(!outbox.queue(mail("late@example.org")));
}

#[tokio::test]
async fn a_message_that_waited_too_long_is_dropped() {
    let recorder = Recorder::new(0);
    let outbox = Outbox::with_limits(recorder.clone(), 64, Duration::from_millis(50));
    // More than can be sent at once, so the last ones wait behind a relay that stalls.
    for n in 0..20 {
        assert!(outbox.queue(mail(&format!("user{n}@example.org"))));
    }
    until("some sends start", || recorder.started() >= 1).await;
    let stalled = recorder.started();
    assert!(stalled < 20, "all {stalled} were sent at once");
    tokio::time::sleep(Duration::from_millis(100)).await;

    recorder.gate.add_permits(100);
    assert!(outbox.queue(mail("fresh@example.org")));
    until("the fresh message goes out", || {
        recorder.sent().contains(&"fresh@example.org".to_owned())
    })
    .await;
    // Those that had started went out, the ones still queued were too old by then.
    assert_eq!(recorder.sent().len(), stalled + 1, "{:?}", recorder.sent());
}

#[tokio::test]
async fn a_reserved_place_is_kept_until_its_message_comes() {
    let recorder = Recorder::new(0);
    let outbox = Outbox::with_limits(recorder.clone(), 2, Duration::from_secs(60));
    let place = outbox.reserve().expect("room in an empty outbox");
    // Filled again as the task takes messages out, one per round, until nothing moves any more.
    for _ in 0..12 {
        for n in 0..50 {
            outbox.queue(mail(&format!("user{n}@example.org")));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        outbox.reserve().is_none(),
        "a full outbox has no place left"
    );

    place.send(mail("ada@example.org"));
    recorder.gate.add_permits(100);
    until("the reserved message goes out", || {
        recorder.sent().contains(&"ada@example.org".to_owned())
    })
    .await;
}
