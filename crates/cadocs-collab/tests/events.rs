//! The workspace hub behind events sockets, with no socket: `cadocs-server` only moves its frames.

mod common;

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cadocs_collab::{
    Authorizer, CollabConfig, CollabError, DocRegistry, Event, EventConnection, EventOutbound,
    EventSeat, EventStanding, PageEvent, Seat, SpaceEvent,
};
use cadocs_core::permissions::SocketMode::ReadWrite;
use cadocs_core::permissions::{SocketMode, SocketOutcome, SpaceRole};
use cadocs_store::StoreError;
use common::MemStore;
use tokio::time::timeout;

const WS: &str = "ws";
const OTHER_WS: &str = "other-ws";
const ADA: &str = "ada";
const SESSION: &str = "session";
const QUEUE: usize = 4;

fn registry() -> DocRegistry<MemStore> {
    let config = CollabConfig {
        flush_interval: Duration::from_millis(10),
        send_queue: QUEUE,
        ..CollabConfig::default()
    };
    DocRegistry::new(Arc::new(MemStore::default()), config)
}

fn spaces(ids: &[&str]) -> Vec<(String, SpaceRole)> {
    let reader = |id: &&str| ((*id).to_owned(), SpaceRole::Viewer);
    ids.iter().map(reader).collect()
}

fn join(registry: &DocRegistry<MemStore>, ids: &[&str]) -> EventConnection {
    registry
        .join_events(WS, ADA, SESSION, spaces(ids))
        .expect("a seat on the events socket")
}

fn moved(space: &str) -> Event {
    Event::page(PageEvent::Moved, space, "page")
}

fn moved_frame(space: &str) -> EventOutbound {
    let frame = format!(r#"{{"page":"page","space":"{space}","type":"page.moved"}}"#);
    EventOutbound::Text(frame)
}

const ACCESS_CHANGED: &str = r#"{"type":"access.changed"}"#;
const RESYNC: &str = r#"{"type":"resync"}"#;

fn text(frame: &str) -> EventOutbound {
    EventOutbound::Text(frame.to_owned())
}

async fn next(connection: &mut EventConnection) -> EventOutbound {
    let waited = timeout(Duration::from_secs(5), connection.next()).await;
    waited.expect("a frame")
}

async fn silent(connection: &mut EventConnection) {
    let waited = timeout(Duration::from_millis(50), connection.next()).await;
    assert_eq!(waited.ok(), None);
}

/// Answers with what a test set, whoever asks.
#[derive(Default)]
struct Scripted {
    standing: Mutex<Option<EventStanding>>,
    fails: AtomicBool,
}

impl Scripted {
    fn answers(standing: EventStanding) -> Self {
        Self {
            standing: Mutex::new(Some(standing)),
            fails: AtomicBool::new(false),
        }
    }
}

impl Authorizer for Scripted {
    async fn authorize(&self, _: &str, _: &str) -> Result<Option<SocketMode>, StoreError> {
        Ok(Some(ReadWrite))
    }

    async fn recheck(&self, _seat: &Seat<'_>) -> Result<SocketOutcome, StoreError> {
        Ok(SocketOutcome::Keep)
    }

    async fn recheck_events(&self, seat: &EventSeat<'_>) -> Result<EventStanding, StoreError> {
        assert_eq!(
            (seat.workspace_id, seat.user_id, seat.session),
            (WS, ADA, SESSION)
        );
        if self.fails.load(Ordering::SeqCst) {
            return Err(StoreError::PageNotFound);
        }
        Ok(self.standing.lock().unwrap().clone().unwrap())
    }
}

#[tokio::test]
async fn a_socket_hears_the_spaces_it_reads_in_its_workspace() {
    let registry = registry();
    let mut connection = join(&registry, &["a", "c"]);

    registry.publish(WS, moved("b"));
    registry.publish(OTHER_WS, moved("a"));
    registry.publish(WS, moved("c"));

    assert_eq!(next(&mut connection).await, moved_frame("c"));
    silent(&mut connection).await;
}

#[tokio::test]
async fn frames_are_the_events_of_the_api() {
    let registry = registry();
    let mut connection = join(&registry, &["a"]);

    registry.publish(WS, Event::page(PageEvent::Created, "a", "page"));
    registry.publish(WS, Event::page(PageEvent::Renamed, "a", "page"));
    registry.publish(WS, Event::space(SpaceEvent::Updated, "a"));

    let expected = [
        r#"{"page":"page","space":"a","type":"page.created"}"#,
        r#"{"page":"page","space":"a","type":"page.renamed"}"#,
        r#"{"space":"a","type":"space.updated"}"#,
    ];
    for frame in expected {
        assert_eq!(next(&mut connection).await, text(frame));
    }
}

#[tokio::test]
async fn a_lagging_socket_gets_one_resync_and_then_what_comes_next() {
    let registry = registry();
    let mut lagging = join(&registry, &["a"]);

    for _ in 0..QUEUE * 3 {
        registry.publish(WS, moved("a"));
    }
    assert_eq!(next(&mut lagging).await, text(RESYNC));
    // The client reloads everything, so what the queue still held is of no use.
    silent(&mut lagging).await;

    registry.publish(WS, moved("a"));
    assert_eq!(next(&mut lagging).await, moved_frame("a"));
}

#[tokio::test]
async fn a_hub_lives_as_long_as_its_sockets() {
    let registry = registry();
    assert_eq!(registry.event_hubs(), 0);
    let first = join(&registry, &[]);
    let second = join(&registry, &[]);
    assert_eq!(registry.event_hubs(), 1);
    drop(first);
    assert_eq!(registry.event_hubs(), 1);
    drop(second);
    assert_eq!(registry.event_hubs(), 0);
}

#[tokio::test]
async fn events_and_page_sockets_share_one_count() {
    let config = CollabConfig {
        max_sockets: 1,
        ..CollabConfig::default()
    };
    let registry = DocRegistry::new(Arc::new(MemStore::default()), config);

    let connection = join(&registry, &[]);
    let refused = registry.connect("doc", ReadWrite, ADA, SESSION).await;
    assert!(matches!(refused, Err(CollabError::AtCapacity)));
    drop(connection);

    let page = registry.connect("doc", ReadWrite, ADA, SESSION).await;
    let page = page.unwrap();
    let refused = registry.join_events(WS, ADA, SESSION, Vec::new());
    assert!(matches!(refused, Err(CollabError::AtCapacity)));
    drop(page);
    assert!(registry.join_events(WS, ADA, SESSION, Vec::new()).is_ok());
}

#[tokio::test]
async fn shutdown_closes_events_sockets_and_refuses_new_ones() {
    let registry = registry();
    let mut connection = join(&registry, &[]);
    registry.shutdown().await.unwrap();

    assert_eq!(next(&mut connection).await, EventOutbound::Close(1001));
    let refused = registry.join_events(WS, ADA, SESSION, Vec::new());
    assert!(matches!(refused, Err(CollabError::ShuttingDown)));
}

#[tokio::test]
async fn a_recheck_closes_with_the_code_it_gets() {
    let registry = registry();
    let mut connection = join(&registry, &["a"]);
    let authorizer = Scripted::answers(EventStanding::Close(4403));

    registry.recheck(&authorizer).await.unwrap();

    assert_eq!(next(&mut connection).await, EventOutbound::Close(4403));
}

#[tokio::test]
async fn a_recheck_tells_a_socket_when_its_spaces_change() {
    let registry = registry();
    let mut connection = join(&registry, &["a"]);
    let authorizer = Scripted::answers(EventStanding::Open(spaces(&["b"])));

    registry.recheck(&authorizer).await.unwrap();
    assert_eq!(next(&mut connection).await, text(ACCESS_CHANGED));

    registry.publish(WS, moved("a"));
    registry.publish(WS, moved("b"));
    assert_eq!(next(&mut connection).await, moved_frame("b"));

    registry.recheck(&authorizer).await.unwrap();
    silent(&mut connection).await;
}

#[tokio::test]
async fn a_recheck_tells_a_socket_when_a_role_changes() {
    let registry = registry();
    let mut connection = join(&registry, &["a"]);
    let editor = vec![("a".to_owned(), SpaceRole::Editor)];
    let authorizer = Scripted::answers(EventStanding::Open(editor));

    registry.recheck(&authorizer).await.unwrap();

    assert_eq!(next(&mut connection).await, text(ACCESS_CHANGED));
}

#[tokio::test]
async fn a_socket_that_cannot_be_checked_stays_as_it_is() {
    let registry = registry();
    let mut connection = join(&registry, &["a"]);
    let authorizer = Scripted::default();
    authorizer.fails.store(true, Ordering::SeqCst);

    assert!(registry.recheck(&authorizer).await.is_err());

    registry.publish(WS, moved("a"));
    assert_eq!(next(&mut connection).await, moved_frame("a"));
}

#[tokio::test]
async fn an_event_held_for_the_recheck_reaches_the_readers_it_makes() {
    let registry = registry();
    let mut connection = join(&registry, &["a"]);
    let authorizer = Scripted::answers(EventStanding::Open(spaces(&["a", "new"])));

    registry.publish_after_recheck(WS, Event::space(SpaceEvent::Created, "new"));
    silent(&mut connection).await;
    registry.recheck(&authorizer).await.unwrap();

    let created = text(r#"{"space":"new","type":"space.created"}"#);
    let mut frames = vec![next(&mut connection).await, next(&mut connection).await];
    frames.retain(|frame| *frame != text(ACCESS_CHANGED));
    assert_eq!(frames, [created]);
}

#[tokio::test]
async fn a_flush_that_changes_a_title_announces_it_once() {
    let registry = registry();
    let mut connection = join(&registry, &[common::SPACE]);

    registry.set_title("doc", "Plans", ADA).await.unwrap();
    let renamed = r#"{"page":"doc","space":"space","type":"page.renamed"}"#;
    assert_eq!(next(&mut connection).await, text(renamed));

    registry.set_title("doc", "Plans", ADA).await.unwrap();
    silent(&mut connection).await;
}

#[tokio::test]
async fn the_check_of_a_join_does_not_undo_a_newer_recheck() {
    let registry = registry();
    let mut connection = join(&registry, &["a"]);
    let authorizer = Scripted::answers(EventStanding::Open(spaces(&["b"])));
    registry.recheck(&authorizer).await.unwrap();
    assert_eq!(next(&mut connection).await, text(ACCESS_CHANGED));

    // Read before that pass, and arriving after it.
    connection.settle(EventStanding::Open(spaces(&["a"])));

    registry.publish(WS, moved("a"));
    registry.publish(WS, moved("b"));
    assert_eq!(next(&mut connection).await, moved_frame("b"));
}

#[tokio::test]
async fn the_check_of_a_join_applies_when_no_recheck_came_first() {
    let registry = registry();
    let mut connection = join(&registry, &["a"]);

    connection.settle(EventStanding::Open(spaces(&["b"])));

    assert_eq!(next(&mut connection).await, text(ACCESS_CHANGED));
    registry.publish(WS, moved("b"));
    assert_eq!(next(&mut connection).await, moved_frame("b"));
}

#[tokio::test]
async fn the_check_of_a_join_can_always_close() {
    let registry = registry();
    let mut connection = join(&registry, &["a"]);
    let authorizer = Scripted::answers(EventStanding::Open(spaces(&["b"])));
    registry.recheck(&authorizer).await.unwrap();
    assert_eq!(next(&mut connection).await, text(ACCESS_CHANGED));

    connection.settle(EventStanding::Close(4403));

    assert_eq!(next(&mut connection).await, EventOutbound::Close(4403));
}
