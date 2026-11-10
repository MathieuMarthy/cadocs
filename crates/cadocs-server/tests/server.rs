//! The server over real sockets, with `yrs` standing in for the client. The real `yjs` client is
//! tested in step 4.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Method, Request, StatusCode, header};
use axum::middleware::from_fn_with_state;
use axum::routing::post;
use cadocs_collab::{Authorizer, CollabConfig, DocRegistry, EventSeat, EventStanding, Seat};
use cadocs_core::permissions::{Scope, SocketMode, SocketOutcome, WorkspaceRole};
use cadocs_files::LocalFileStore;
use cadocs_server::api;
use cadocs_server::client_ip::TrustedProxies;
use cadocs_server::config::{AttachmentLimits, HttpConfig, PublicUrl, SocketConfig};
use cadocs_server::rate_limit::RateLimiter;
use cadocs_server::session::{csrf_token, token_hash};
use cadocs_server::{AppState, router, serve};
use cadocs_store::{
    AccountStore, ActiveSession, AdminChanges, Attachment, AttachmentStore, Credentials,
    DateCursor, DocEdit, DocStore, FirstUser, Grant, IndexTarget, Invitation, InvitationPreview,
    InvitedUser, Member, Membership, MovedPage, NewAttachment, NewGrant, NewInvitation, NewPage,
    NewPasswordReset, NewSession, NewSpace, NewWorkspace, PageCrumb, PageIndex, PageMove,
    PageRecord, PageSite, PageStore, SearchHit, SearchStore, SessionStanding, Snapshot, Space,
    SpaceChanges, SpaceStore, SqliteStore, StoreError, StoredDoc, StoredUpdate, UpdateBasis,
    UploadTarget, UserChanges, UserRecord, Workspace, WorkspaceChanges, WorkspaceReach,
    WorkspaceStore, now_ms,
};
use futures_util::{SinkExt, StreamExt};
use rust_embed::Embed;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Request as WsRequest;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tower::ServiceExt;
use yrs::sync::{Message as YMessage, SyncMessage};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, GetString, ReadTxn, StateVector, Text, Transact, Update};

type Client = WebSocketStream<MaybeTlsStream<TcpStream>>;

const PUBLIC_ORIGIN: &str = "https://wiki.example.org";
const TOKEN: &str = "token";

#[derive(Embed)]
#[folder = "tests/fixtures"]
struct Fixtures;

/// A SQLite store whose writes or pings can be made to fail.
#[derive(Clone)]
struct FlakyStore {
    inner: SqliteStore,
    fail_writes: Arc<AtomicBool>,
    fail_ping: Arc<AtomicBool>,
    /// Pauses the next loads until `release_load`, after telling `load_entered`.
    hold_load: Arc<AtomicBool>,
    load_entered: Arc<Notify>,
    release_load: Arc<Notify>,
}

impl DocStore for FlakyStore {
    async fn ping(&self) -> Result<(), StoreError> {
        if self.fail_ping.load(Ordering::SeqCst) {
            return Err(sqlx::Error::PoolTimedOut.into());
        }
        self.inner.ping().await
    }

    async fn load(&self, doc_id: &str) -> Result<StoredDoc, StoreError> {
        if self.hold_load.load(Ordering::SeqCst) {
            let released = self.release_load.notified();
            self.load_entered.notify_one();
            released.await;
        }
        self.inner.load(doc_id).await
    }

    async fn append_updates(
        &self,
        doc_id: &str,
        updates: &[StoredUpdate],
        edit: &DocEdit,
    ) -> Result<Option<PageSite>, StoreError> {
        if self.fail_writes.load(Ordering::SeqCst) {
            return Err(sqlx::Error::PoolTimedOut.into());
        }
        self.inner.append_updates(doc_id, updates, edit).await
    }

    async fn compact(&self, doc_id: &str, snapshot: &Snapshot) -> Result<(), StoreError> {
        self.inner.compact(doc_id, snapshot).await
    }
}

impl PageStore for FlakyStore {
    async fn create_page(&self, page: &NewPage) -> Result<PageRecord, StoreError> {
        self.inner.create_page(page).await
    }

    async fn page(&self, page_id: &str, actor: &str) -> Result<Option<PageRecord>, StoreError> {
        self.inner.page(page_id, actor).await
    }

    async fn page_with_ancestors(
        &self,
        page_id: &str,
        actor: &str,
    ) -> Result<Option<(PageRecord, Vec<PageCrumb>)>, StoreError> {
        self.inner.page_with_ancestors(page_id, actor).await
    }

    async fn page_scope(&self, page_id: &str, actor: &str) -> Result<Option<Scope>, StoreError> {
        self.inner.page_scope(page_id, actor).await
    }

    async fn page_to_edit(&self, page_id: &str, actor: &str) -> Result<PageRecord, StoreError> {
        self.inner.page_to_edit(page_id, actor).await
    }

    async fn page_by_slug(
        &self,
        workspace_id: &str,
        slug: &str,
        actor: &str,
    ) -> Result<Option<PageRecord>, StoreError> {
        self.inner.page_by_slug(workspace_id, slug, actor).await
    }

    async fn tree(
        &self,
        space_id: &str,
        parent_id: Option<&str>,
        actor: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<PageRecord>, StoreError> {
        self.inner
            .tree(space_id, parent_id, actor, after, limit)
            .await
    }

    async fn move_page(
        &self,
        page_id: &str,
        actor: &str,
        destination: &PageMove,
        now: i64,
    ) -> Result<MovedPage, StoreError> {
        self.inner.move_page(page_id, actor, destination, now).await
    }

    async fn trash_page(
        &self,
        page_id: &str,
        actor: &str,
        now: i64,
    ) -> Result<PageSite, StoreError> {
        self.inner.trash_page(page_id, actor, now).await
    }

    async fn restore_page(&self, page_id: &str, actor: &str) -> Result<PageRecord, StoreError> {
        self.inner.restore_page(page_id, actor).await
    }

    async fn purge_page(&self, page_id: &str, actor: &str) -> Result<PageSite, StoreError> {
        self.inner.purge_page(page_id, actor).await
    }

    async fn list_trashed_pages(
        &self,
        space_id: &str,
        actor: &str,
        after: Option<&DateCursor>,
        limit: u32,
    ) -> Result<Vec<PageRecord>, StoreError> {
        self.inner
            .list_trashed_pages(space_id, actor, after, limit)
            .await
    }

    async fn recent_pages(
        &self,
        workspace_id: &str,
        actor: &str,
        after: Option<&DateCursor>,
        limit: u32,
    ) -> Result<Vec<PageRecord>, StoreError> {
        self.inner
            .recent_pages(workspace_id, actor, after, limit)
            .await
    }
}

impl AttachmentStore for FlakyStore {
    async fn upload_target(&self, page_id: &str, actor: &str) -> Result<UploadTarget, StoreError> {
        self.inner.upload_target(page_id, actor).await
    }

    async fn create_attachment(
        &self,
        new: &NewAttachment,
        quota_bytes: Option<i64>,
    ) -> Result<Attachment, StoreError> {
        self.inner.create_attachment(new, quota_bytes).await
    }

    async fn attachment(&self, id: &str, actor: &str) -> Result<Option<Attachment>, StoreError> {
        self.inner.attachment(id, actor).await
    }

    async fn delete_attachment(&self, id: &str, actor: &str) -> Result<(), StoreError> {
        self.inner.delete_attachment(id, actor).await
    }

    async fn orphan_attachments(&self, limit: u32) -> Result<Vec<String>, StoreError> {
        self.inner.orphan_attachments(limit).await
    }

    async fn forget_orphan(&self, id: &str) -> Result<bool, StoreError> {
        self.inner.forget_orphan(id).await
    }

    async fn known_attachments(&self, ids: &[String]) -> Result<Vec<String>, StoreError> {
        self.inner.known_attachments(ids).await
    }
}

impl SearchStore for FlakyStore {
    async fn index_target(&self, page_id: &str) -> Result<Option<IndexTarget>, StoreError> {
        self.inner.index_target(page_id).await
    }

    async fn index_page(&self, index: &PageIndex<'_>) -> Result<bool, StoreError> {
        self.inner.index_page(index).await
    }

    async fn search(
        &self,
        workspace_id: &str,
        actor: &str,
        fts: &str,
        space: Option<&str>,
        limit: u32,
    ) -> Result<Vec<SearchHit>, StoreError> {
        self.inner
            .search(workspace_id, actor, fts, space, limit)
            .await
    }
}

impl SpaceStore for FlakyStore {
    async fn create_space(&self, space: &NewSpace) -> Result<Space, StoreError> {
        self.inner.create_space(space).await
    }

    async fn list_spaces(
        &self,
        workspace_id: &str,
        actor: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Space>, StoreError> {
        self.inner
            .list_spaces(workspace_id, actor, after, limit)
            .await
    }

    async fn list_trashed_spaces(
        &self,
        workspace_id: &str,
        actor: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Space>, StoreError> {
        let inner = &self.inner;
        inner
            .list_trashed_spaces(workspace_id, actor, after, limit)
            .await
    }

    async fn space(&self, space_id: &str, actor: &str) -> Result<Option<Space>, StoreError> {
        self.inner.space(space_id, actor).await
    }

    async fn update_space(
        &self,
        space_id: &str,
        actor: &str,
        changes: &SpaceChanges,
        now: i64,
    ) -> Result<Space, StoreError> {
        self.inner.update_space(space_id, actor, changes, now).await
    }

    async fn trash_space(
        &self,
        space_id: &str,
        actor: &str,
        now: i64,
    ) -> Result<String, StoreError> {
        self.inner.trash_space(space_id, actor, now).await
    }

    async fn workspace_reach(
        &self,
        workspace_id: &str,
        user_id: &str,
    ) -> Result<WorkspaceReach, StoreError> {
        self.inner.workspace_reach(workspace_id, user_id).await
    }

    async fn restore_space(
        &self,
        space_id: &str,
        actor: &str,
        now: i64,
    ) -> Result<Space, StoreError> {
        self.inner.restore_space(space_id, actor, now).await
    }

    async fn list_grants(
        &self,
        space_id: &str,
        actor: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Grant>, StoreError> {
        self.inner.list_grants(space_id, actor, after, limit).await
    }

    async fn set_grants(
        &self,
        space_id: &str,
        actor: &str,
        grants: &[NewGrant],
        now: i64,
    ) -> Result<(), StoreError> {
        self.inner.set_grants(space_id, actor, grants, now).await
    }
}

impl WorkspaceStore for FlakyStore {
    async fn create_workspace(&self, workspace: &NewWorkspace) -> Result<Workspace, StoreError> {
        self.inner.create_workspace(workspace).await
    }

    async fn list_workspaces(
        &self,
        user_id: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Membership>, StoreError> {
        self.inner.list_workspaces(user_id, after, limit).await
    }

    async fn workspace(&self, id: &str, user_id: &str) -> Result<Option<Workspace>, StoreError> {
        self.inner.workspace(id, user_id).await
    }

    async fn update_workspace(
        &self,
        id: &str,
        actor: &str,
        changes: &WorkspaceChanges,
        now: i64,
    ) -> Result<Workspace, StoreError> {
        self.inner.update_workspace(id, actor, changes, now).await
    }

    async fn delete_workspace(
        &self,
        id: &str,
        actor: &str,
        confirmation: &str,
    ) -> Result<(), StoreError> {
        self.inner.delete_workspace(id, actor, confirmation).await
    }

    async fn list_members(
        &self,
        id: &str,
        actor: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Member>, StoreError> {
        self.inner.list_members(id, actor, after, limit).await
    }

    async fn set_member_role(
        &self,
        id: &str,
        actor: &str,
        user_id: &str,
        role: WorkspaceRole,
    ) -> Result<Member, StoreError> {
        self.inner.set_member_role(id, actor, user_id, role).await
    }

    async fn remove_member(&self, id: &str, actor: &str, user_id: &str) -> Result<(), StoreError> {
        self.inner.remove_member(id, actor, user_id).await
    }

    async fn create_invitation(&self, invitation: &NewInvitation) -> Result<(), StoreError> {
        self.inner.create_invitation(invitation).await
    }

    async fn list_invitations(
        &self,
        id: &str,
        actor: &str,
        now: i64,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Invitation>, StoreError> {
        self.inner
            .list_invitations(id, actor, now, after, limit)
            .await
    }

    async fn revoke_invitation(&self, id: &str, actor: &str) -> Result<(), StoreError> {
        self.inner.revoke_invitation(id, actor).await
    }

    async fn invitation(
        &self,
        token_hash: &str,
        now: i64,
    ) -> Result<Option<InvitationPreview>, StoreError> {
        self.inner.invitation(token_hash, now).await
    }

    async fn accept_as_new_user(
        &self,
        token_hash: &str,
        user: &InvitedUser,
        session: &NewSession,
    ) -> Result<Vec<Membership>, StoreError> {
        self.inner
            .accept_as_new_user(token_hash, user, session)
            .await
    }

    async fn accept_as_user(
        &self,
        token_hash: &str,
        user_id: &str,
        now: i64,
    ) -> Result<Vec<Membership>, StoreError> {
        self.inner.accept_as_user(token_hash, user_id, now).await
    }
}

impl AccountStore for FlakyStore {
    async fn has_users(&self) -> Result<bool, StoreError> {
        self.inner.has_users().await
    }

    async fn create_first_user(
        &self,
        user: &FirstUser,
        session: &NewSession,
    ) -> Result<(), StoreError> {
        self.inner.create_first_user(user, session).await
    }

    async fn credentials(&self, email: &str) -> Result<Option<Credentials>, StoreError> {
        self.inner.credentials(email).await
    }

    async fn password_hash(&self, user_id: &str) -> Result<Option<String>, StoreError> {
        self.inner.password_hash(user_id).await
    }

    async fn create_session(
        &self,
        session: &NewSession,
        checked_hash: &str,
    ) -> Result<bool, StoreError> {
        self.inner.create_session(session, checked_hash).await
    }

    async fn session(
        &self,
        token_hash: &str,
        now: i64,
    ) -> Result<Option<ActiveSession>, StoreError> {
        if self.fail_ping.load(Ordering::SeqCst) {
            return Err(sqlx::Error::PoolTimedOut.into());
        }
        self.inner.session(token_hash, now).await
    }

    async fn session_standing(
        &self,
        token_hash: &str,
        user_id: &str,
        now: i64,
    ) -> Result<SessionStanding, StoreError> {
        self.inner.session_standing(token_hash, user_id, now).await
    }

    async fn touch_session(
        &self,
        token_hash: &str,
        now: i64,
        expires_at: i64,
    ) -> Result<(), StoreError> {
        if self.fail_writes.load(Ordering::SeqCst) {
            return Err(sqlx::Error::PoolTimedOut.into());
        }
        self.inner.touch_session(token_hash, now, expires_at).await
    }

    async fn delete_session(&self, token_hash: &str) -> Result<(), StoreError> {
        self.inner.delete_session(token_hash).await
    }

    async fn memberships(&self, user_id: &str) -> Result<Vec<Membership>, StoreError> {
        self.inner.memberships(user_id).await
    }

    async fn update_user(
        &self,
        user_id: &str,
        changes: &UserChanges,
        basis: &UpdateBasis<'_>,
        now: i64,
    ) -> Result<(), StoreError> {
        self.inner.update_user(user_id, changes, basis, now).await
    }

    async fn delete_user(&self, user_id: &str) -> Result<(), StoreError> {
        self.inner.delete_user(user_id).await
    }

    async fn list_users(
        &self,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<UserRecord>, StoreError> {
        self.inner.list_users(after, limit).await
    }

    async fn administer_user(
        &self,
        user_id: &str,
        changes: AdminChanges,
        now: i64,
    ) -> Result<UserRecord, StoreError> {
        self.inner.administer_user(user_id, changes, now).await
    }

    async fn create_password_reset(&self, reset: &NewPasswordReset) -> Result<(), StoreError> {
        self.inner.create_password_reset(reset).await
    }

    async fn password_reset_usable(&self, token_hash: &str, now: i64) -> Result<bool, StoreError> {
        self.inner.password_reset_usable(token_hash, now).await
    }

    async fn use_password_reset(
        &self,
        token_hash: &str,
        password_hash: &str,
        now: i64,
    ) -> Result<bool, StoreError> {
        self.inner
            .use_password_reset(token_hash, password_hash, now)
            .await
    }
}

#[derive(Clone, Copy)]
struct Fixed(Option<SocketMode>);

/// Gives every user write access, for tests about something else.
const WRITER: Fixed = Fixed(Some(SocketMode::ReadWrite));

impl Authorizer for Fixed {
    async fn authorize(
        &self,
        _page_id: &str,
        _user_id: &str,
    ) -> Result<Option<SocketMode>, StoreError> {
        Ok(self.0)
    }

    async fn recheck(&self, _seat: &Seat<'_>) -> Result<SocketOutcome, StoreError> {
        Ok(SocketOutcome::Keep)
    }

    async fn recheck_events(&self, _: &EventSeat<'_>) -> Result<EventStanding, StoreError> {
        Ok(EventStanding::Open(Vec::new()))
    }
}

/// Allows the upgrade, then answers the check made once the socket has joined.
struct ChangedDuringTheJoin(Option<SocketOutcome>);

impl Authorizer for ChangedDuringTheJoin {
    async fn authorize(&self, _: &str, _: &str) -> Result<Option<SocketMode>, StoreError> {
        Ok(Some(SocketMode::ReadWrite))
    }

    async fn recheck(&self, _seat: &Seat<'_>) -> Result<SocketOutcome, StoreError> {
        self.0.ok_or(StoreError::PageNotFound)
    }

    async fn recheck_events(&self, _: &EventSeat<'_>) -> Result<EventStanding, StoreError> {
        Ok(EventStanding::Open(Vec::new()))
    }
}

/// Panics in its first pass over the sockets, and refuses them in the next one.
#[derive(Default)]
struct PanicsOnce(AtomicUsize);

impl Authorizer for PanicsOnce {
    async fn authorize(&self, _: &str, _: &str) -> Result<Option<SocketMode>, StoreError> {
        Ok(Some(SocketMode::ReadWrite))
    }

    async fn recheck(&self, _seat: &Seat<'_>) -> Result<SocketOutcome, StoreError> {
        match self.0.fetch_add(1, Ordering::SeqCst) {
            // The check of the socket that joins.
            0 => Ok(SocketOutcome::Keep),
            1 => panic!("a pass that fails badly"),
            _ => Ok(SocketOutcome::Close(4403)),
        }
    }

    async fn recheck_events(&self, _: &EventSeat<'_>) -> Result<EventStanding, StoreError> {
        Ok(EventStanding::Open(Vec::new()))
    }
}

fn collab() -> CollabConfig {
    CollabConfig {
        flush_interval: Duration::from_millis(10),
        ..CollabConfig::default()
    }
}

fn http() -> HttpConfig {
    HttpConfig {
        public_url: PublicUrl::parse(PUBLIC_ORIGIN).unwrap(),
        trusted_proxies: TrustedProxies::default(),
        session_lifetime: Duration::from_secs(60),
    }
}

/// The user behind every socket of these tests, and the page they open. A restart finds them.
async fn seed(store: &SqliteStore, path: &Path) {
    if store.has_users().await.unwrap() {
        return;
    }
    let user = FirstUser {
        id: "user".to_owned(),
        email: "ada@example.org".to_owned(),
        name: "Ada".to_owned(),
        password_hash: "unused".to_owned(),
        workspace_id: "workspace".to_owned(),
        workspace_slug: "workspace".to_owned(),
        workspace_name: "Workspace".to_owned(),
    };
    let session = NewSession {
        token_hash: token_hash(TOKEN),
        user_id: user.id.clone(),
        now: now_ms(),
        expires_at: i64::MAX,
    };
    store.create_first_user(&user, &session).await.unwrap();
    let pool = SqlitePool::connect_with(SqliteConnectOptions::new().filename(path))
        .await
        .unwrap();
    sqlx::raw_sql(
        "INSERT INTO spaces (id, workspace_id, slug, name, created_at, updated_at)
             VALUES ('space', 'workspace', 'space', 'Space', 1, 1);
         INSERT INTO pages (id, workspace_id, space_id, position, slug, created_at, updated_at)
             VALUES ('doc', 'workspace', 'space', 'a', 'doc', 1, 1);",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
}

async fn state<A: Authorizer>(
    dir: &TempDir,
    collab: CollabConfig,
    socket: SocketConfig,
    authorizer: A,
) -> AppState<FlakyStore, A> {
    let path = dir.path().join("cadocs.db");
    let inner = SqliteStore::open(&path).await.unwrap();
    seed(&inner, &path).await;
    let store = Arc::new(FlakyStore {
        inner,
        fail_writes: Arc::default(),
        fail_ping: Arc::default(),
        hold_load: Arc::default(),
        load_entered: Arc::default(),
        release_load: Arc::default(),
    });
    let registry = DocRegistry::new(store.clone(), collab);
    let files = Arc::new(LocalFileStore::open(dir.path()).unwrap());
    let limits = AttachmentLimits::default();
    AppState::new(
        registry,
        store,
        Arc::new(authorizer),
        socket,
        http(),
        files,
        limits,
    )
}

struct Running {
    addr: std::net::SocketAddr,
    stop: oneshot::Sender<()>,
    task: JoinHandle<anyhow::Result<()>>,
}

impl Running {
    /// The request of a logged-in client.
    fn request(&self, doc: &str) -> WsRequest {
        let url = format!("ws://{}/ws/pages/{doc}", self.addr);
        let mut request = url.into_client_request().unwrap();
        let cookie = format!("session={TOKEN}").parse().unwrap();
        request.headers_mut().insert(header::COOKIE, cookie);
        request
    }

    async fn client(&self, doc: &str) -> Client {
        connect_async(self.request(doc)).await.unwrap().0
    }

    async fn stop(self) {
        self.stop.send(()).unwrap();
        timeout(Duration::from_secs(10), self.task)
            .await
            .expect("the server must stop")
            .unwrap()
            .unwrap();
    }
}

async fn start<A: Authorizer>(state: AppState<FlakyStore, A>) -> Running {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(serve::<_, _, Fixtures>(listener, state, async {
        let _ = stopped.await;
    }));
    Running { addr, stop, task }
}

async fn start_default(dir: &TempDir) -> Running {
    start(state(dir, collab(), SocketConfig::default(), WRITER).await).await
}

async fn get<A: Authorizer>(
    state: AppState<FlakyStore, A>,
    path: &str,
) -> axum::response::Response {
    router::<_, _, Fixtures>(state)
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn body_json(response: axum::response::Response) -> Value {
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn sync_step1(doc: &Doc) -> Vec<u8> {
    let state_vector = doc.transact().state_vector();
    YMessage::Sync(SyncMessage::SyncStep1(state_vector)).encode_v1()
}

fn update_frame(text: &str) -> Vec<u8> {
    let doc = Doc::new();
    let title = doc.get_or_insert_text("title");
    let mut txn = doc.transact_mut();
    title.push(&mut txn, text);
    let update = txn.encode_state_as_update_v1(&StateVector::default());
    YMessage::Sync(SyncMessage::Update(update)).encode_v1()
}

/// Applies binary frames until `title` reads `expected`.
async fn wait_for_title(client: &mut Client, doc: &Doc, expected: &str) {
    let title = doc.get_or_insert_text("title");
    let result = timeout(Duration::from_secs(5), async {
        loop {
            let Message::Binary(frame) = client.next().await.unwrap().unwrap() else {
                continue;
            };
            if let Ok(YMessage::Sync(
                SyncMessage::SyncStep2(update) | SyncMessage::Update(update),
            )) = YMessage::decode_v1(&frame)
            {
                doc.transact_mut()
                    .apply_update(Update::decode_v1(&update).unwrap())
                    .unwrap();
            }
            if title.get_string(&doc.transact()) == expected {
                return;
            }
        }
    })
    .await;
    assert!(result.is_ok(), "title never became {expected:?}");
}

/// Reads until the socket closes, and returns the close code.
async fn close_code(client: &mut Client) -> Option<CloseCode> {
    timeout(Duration::from_secs(5), async {
        loop {
            match client.next().await {
                Some(Ok(Message::Close(frame))) => return frame.map(|frame| frame.code),
                Some(Ok(_)) => {}
                Some(Err(_)) | None => return None,
            }
        }
    })
    .await
    .expect("the socket must close")
}

async fn refused_status(request: impl IntoClientRequest + Unpin) -> StatusCode {
    match connect_async(request).await {
        Err(WsError::Http(response)) => response.status(),
        Err(error) => panic!("unexpected error: {error}"),
        Ok(_) => panic!("the upgrade must be refused"),
    }
}

#[tokio::test]
async fn health_is_ok_on_a_working_server() {
    let dir = TempDir::new().unwrap();
    let state = state(&dir, collab(), SocketConfig::default(), WRITER).await;
    assert_eq!(get(state, "/health").await.status(), StatusCode::OK);
}

#[tokio::test]
async fn health_fails_when_the_database_does_not_answer() {
    let dir = TempDir::new().unwrap();
    let state = state(&dir, collab(), SocketConfig::default(), WRITER).await;
    state.store.fail_ping.store(true, Ordering::SeqCst);
    assert_eq!(
        get(state, "/health").await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

/// A session in the database of `state`, and the cookie that opens it.
async fn open_session(state: &AppState<FlakyStore, Fixed>, last_seen_at: i64) -> String {
    let session = NewSession {
        token_hash: token_hash("other"),
        user_id: "user".to_owned(),
        now: last_seen_at,
        expires_at: i64::MAX,
    };
    assert!(
        state
            .store
            .create_session(&session, "unused")
            .await
            .unwrap()
    );
    "session=other".to_owned()
}

async fn get_session(state: AppState<FlakyStore, Fixed>, cookie: &str) -> axum::response::Response {
    let request = Request::get("/api/v1/session")
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    router::<_, _, Fixtures>(state)
        .oneshot(request)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_failing_database_is_not_a_logout() {
    let dir = TempDir::new().unwrap();
    let state = state(&dir, collab(), SocketConfig::default(), WRITER).await;
    let cookie = open_session(&state, now_ms()).await;
    state.store.fail_ping.store(true, Ordering::SeqCst);

    let response = get_session(state, &cookie).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body_json(response).await["error"]["code"], "unavailable");
}

#[tokio::test]
async fn a_session_that_cannot_be_extended_still_works() {
    let dir = TempDir::new().unwrap();
    let state = state(&dir, collab(), SocketConfig::default(), WRITER).await;
    let cookie = open_session(&state, 0).await;
    state.store.fail_writes.store(true, Ordering::SeqCst);

    let response = get_session(state, &cookie).await;
    assert_eq!(response.status(), StatusCode::OK);
    // The browser must not outlive the row.
    assert!(response.headers().get(header::SET_COOKIE).is_none());
}

#[tokio::test]
async fn sqlite_ping_reads_the_database() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("cadocs.db");
    let store = SqliteStore::open(&path).await.unwrap();
    store.ping().await.unwrap();

    let dir = TempDir::new().unwrap();
    let path = dir.path().join("cadocs.db");
    let store = SqliteStore::open(&path).await.unwrap();
    // The first ping is what opens the read connection, which now finds no database.
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    assert!(store.ping().await.is_err());
}

#[tokio::test]
async fn health_fails_while_flushes_fail() {
    let dir = TempDir::new().unwrap();
    let state = state(&dir, collab(), SocketConfig::default(), WRITER).await;
    state.store.fail_writes.store(true, Ordering::SeqCst);
    let server = start(state.clone()).await;
    let mut client = server.client("doc").await;
    client
        .send(Message::binary(update_frame("lost")))
        .await
        .unwrap();

    let failing = timeout(Duration::from_secs(5), async {
        while state.registry.healthy() {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(failing.is_ok(), "the flush failure must be reported");
    assert_eq!(
        get(state.clone(), "/health").await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    state.store.fail_writes.store(false, Ordering::SeqCst);
    server.stop().await;
}

#[tokio::test]
async fn the_root_serves_the_page_with_its_security_headers() {
    let dir = TempDir::new().unwrap();
    let state = state(&dir, collab(), SocketConfig::default(), WRITER).await;
    let response = get(state, "/").await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers();
    assert_eq!(headers[header::CONTENT_TYPE], "text/html; charset=utf-8");
    assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
    assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(headers[header::REFERRER_POLICY], "same-origin");
    assert_eq!(
        headers[header::CONTENT_SECURITY_POLICY],
        "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; \
         connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"
    );
}

#[tokio::test]
async fn every_kind_of_response_has_the_security_headers() {
    let dir = TempDir::new().unwrap();
    for path in [
        "/",
        "/missing.js",
        "/health",
        "/api/v1/missing",
        "/ws/pages/doc",
    ] {
        let state = state(&dir, collab(), SocketConfig::default(), WRITER).await;
        let response = get(state, path).await;
        let headers = response.headers();
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff", "{path}");
        assert_eq!(headers[header::REFERRER_POLICY], "same-origin", "{path}");
        assert!(
            headers.contains_key(header::CONTENT_SECURITY_POLICY),
            "{path}"
        );
    }
}

#[tokio::test]
async fn a_handler_keeps_its_own_security_policy() {
    let strict = "script-src 'none'";
    let app = Router::new()
        .route(
            "/",
            axum::routing::get(move || async move {
                ([(header::CONTENT_SECURITY_POLICY, strict)], "public page")
            }),
        )
        .layer(axum::middleware::map_response(
            cadocs_server::security::add_headers,
        ));
    let response = app
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.headers()[header::CONTENT_SECURITY_POLICY], strict);
    assert_eq!(
        response.headers()[header::X_CONTENT_TYPE_OPTIONS],
        "nosniff"
    );
}

#[tokio::test]
async fn unknown_api_routes_answer_in_the_error_shape() {
    let dir = TempDir::new().unwrap();
    for path in ["/api/v1/missing", "/api/v1/a/b", "/api/v1/", "/api/v1"] {
        let state = state(&dir, collab(), SocketConfig::default(), WRITER).await;
        let response = get(state, path).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/json",
            "{path}"
        );
        assert_eq!(
            body_json(response).await,
            json!({ "error": { "code": "not_found", "message": "Not found" } }),
            "{path}"
        );
    }
}

#[tokio::test]
async fn a_foreign_origin_gets_a_403_from_the_api() {
    let dir = TempDir::new().unwrap();
    let app =
        router::<_, _, Fixtures>(state(&dir, collab(), SocketConfig::default(), WRITER).await);
    let request = |origin: &str| {
        Request::post("/api/v1/missing")
            .header(header::ORIGIN, origin)
            .body(Body::empty())
            .unwrap()
    };

    let refused = app
        .clone()
        .oneshot(request("https://evil.example"))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(body_json(refused).await["error"]["code"], "forbidden");

    let allowed = app.oneshot(request(PUBLIC_ORIGIN)).await.unwrap();
    assert_eq!(allowed.status(), StatusCode::NOT_FOUND);
}

/// A route behind a limiter of two requests, as `peer` would reach it.
fn limited(proxies: &str, peer: [u8; 4]) -> Router {
    let limiter = Arc::new(RateLimiter::new(2, Duration::from_secs(90)));
    let http = Arc::new(HttpConfig {
        trusted_proxies: TrustedProxies::parse(proxies).unwrap(),
        ..http()
    });
    Router::new()
        .route("/login", post(|| async { "welcome" }))
        .route_layer(from_fn_with_state((limiter, http), api::rate_limit))
        .layer(MockConnectInfo(std::net::SocketAddr::from((peer, 1234))))
}

async fn login(app: &Router, forwarded_for: Option<&str>) -> axum::response::Response {
    let mut request = Request::builder().method(Method::POST).uri("/login");
    if let Some(client) = forwarded_for {
        request = request.header("x-forwarded-for", client);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn past_the_limit_a_client_gets_429_and_a_delay() {
    let app = limited("", [203, 0, 113, 9]);
    for _ in 0..2 {
        assert_eq!(login(&app, None).await.status(), StatusCode::OK);
    }

    let refused = login(&app, None).await;
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.headers()[header::RETRY_AFTER], "90");
    assert_eq!(body_json(refused).await["error"]["code"], "rate_limited");
}

#[tokio::test]
async fn behind_a_trusted_proxy_each_client_has_its_own_limit() {
    let app = limited("10.0.0.1", [10, 0, 0, 1]);
    for _ in 0..2 {
        assert_eq!(
            login(&app, Some("198.51.100.1")).await.status(),
            StatusCode::OK
        );
    }
    let refused = login(&app, Some("198.51.100.1")).await.status();
    assert_eq!(refused, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        login(&app, Some("198.51.100.2")).await.status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_client_cannot_dodge_the_limit_with_a_forged_header() {
    let app = limited("", [203, 0, 113, 9]);
    for n in 0..2 {
        let forged = format!("198.51.100.{n}");
        assert_eq!(login(&app, Some(&forged)).await.status(), StatusCode::OK);
    }
    let refused = login(&app, Some("198.51.100.9")).await.status();
    assert_eq!(refused, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn hashed_assets_are_cached_for_good() {
    let dir = TempDir::new().unwrap();
    let state = state(&dir, collab(), SocketConfig::default(), WRITER).await;

    let script = get(state.clone(), "/assets/app-abc123.js").await;
    assert_eq!(script.status(), StatusCode::OK);
    assert_eq!(
        script.headers()[header::CONTENT_TYPE],
        "text/javascript; charset=utf-8"
    );
    assert_eq!(
        script.headers()[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );

    let font = get(state, "/assets/font-abc123.woff2").await;
    assert_eq!(font.headers()[header::CONTENT_TYPE], "font/woff2");
}

#[tokio::test]
async fn unknown_paths_and_methods_are_refused() {
    let dir = TempDir::new().unwrap();
    let state = state(&dir, collab(), SocketConfig::default(), WRITER).await;
    assert_eq!(
        get(state.clone(), "/missing.js").await.status(),
        StatusCode::NOT_FOUND
    );
    let post = router::<_, _, Fixtures>(state)
        .oneshot(Request::post("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn front_end_routes_get_the_page() {
    let dir = TempDir::new().unwrap();
    let state = state(&dir, collab(), SocketConfig::default(), WRITER).await;
    let index = Fixtures::get("index.html").unwrap();

    for path in [
        "/login",
        "/invite/token",
        "/reset-password/token",
        "/w/docs",
    ] {
        let response = get(state.clone(), path).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
        let body = axum::body::to_bytes(response.into_body(), usize::MAX);
        assert_eq!(body.await.unwrap().as_ref(), index.data.as_ref(), "{path}");
    }
}

#[tokio::test]
async fn missing_files_stay_missing() {
    let dir = TempDir::new().unwrap();
    let state = state(&dir, collab(), SocketConfig::default(), WRITER).await;
    for path in [
        "/assets/gone",
        "/assets/gone.js",
        "/licenses/gone.txt",
        "/ws/gone",
    ] {
        let response = get(state.clone(), path).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn two_clients_edit_one_document() {
    let dir = TempDir::new().unwrap();
    let server = start_default(&dir).await;
    let mut alice = server.client("doc").await;
    let mut bob = server.client("doc").await;

    alice
        .send(Message::binary(update_frame("hello")))
        .await
        .unwrap();

    wait_for_title(&mut bob, &Doc::new(), "hello").await;
    server.stop().await;
}

#[tokio::test]
async fn edits_survive_a_server_restart() {
    let dir = TempDir::new().unwrap();
    // No periodic flush will run, so only the flush at shutdown can save the edit.
    let collab = CollabConfig {
        flush_interval: Duration::from_secs(3600),
        ..collab()
    };
    let server = start(state(&dir, collab, SocketConfig::default(), WRITER).await).await;
    let mut client = server.client("doc").await;
    client
        .send(Message::binary(update_frame("kept")))
        .await
        .unwrap();
    // The frame must reach the room before the shutdown does.
    client
        .send(Message::binary(sync_step1(&Doc::new())))
        .await
        .unwrap();
    wait_for_title(&mut client, &Doc::new(), "kept").await;
    server.stop().await;

    let server = start_default(&dir).await;
    let mut client = server.client("doc").await;
    let doc = Doc::new();
    client
        .send(Message::binary(sync_step1(&doc)))
        .await
        .unwrap();
    wait_for_title(&mut client, &doc, "kept").await;
    server.stop().await;
}

#[tokio::test]
async fn a_change_during_the_join_closes_the_socket_with_its_code() {
    let dir = TempDir::new().unwrap();
    let outcome = Some(SocketOutcome::Close(4404));
    let authorizer = ChangedDuringTheJoin(outcome);
    let state = state(&dir, collab(), SocketConfig::default(), authorizer).await;
    let server = start(state).await;
    let mut client = server.client("doc").await;
    assert_eq!(close_code(&mut client).await, Some(CloseCode::from(4404)));
}

#[tokio::test]
async fn a_socket_that_cannot_be_checked_after_the_join_is_refused() {
    let dir = TempDir::new().unwrap();
    let authorizer = ChangedDuringTheJoin(None);
    let state = state(&dir, collab(), SocketConfig::default(), authorizer).await;
    let server = start(state).await;
    let status = refused_status(server.request("doc")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn a_pass_that_panics_does_not_stop_the_next_ones() {
    let dir = TempDir::new().unwrap();
    let socket = SocketConfig {
        recheck: Duration::from_millis(20),
        ..SocketConfig::default()
    };
    let state = state(&dir, collab(), socket, PanicsOnce::default()).await;
    let server = start(state).await;
    let mut client = server.client("doc").await;
    assert_eq!(close_code(&mut client).await, Some(CloseCode::from(4403)));
}

#[tokio::test]
async fn shutdown_closes_sockets_with_going_away() {
    let dir = TempDir::new().unwrap();
    let server = start_default(&dir).await;
    let mut client = server.client("doc").await;
    server.stop().await;
    assert_eq!(close_code(&mut client).await, Some(CloseCode::Away));
}

#[tokio::test]
async fn a_stalled_http_client_does_not_block_shutdown() {
    let dir = TempDir::new().unwrap();
    let server = start_default(&dir).await;
    let mut client = server.client("doc").await;
    let mut stalled = TcpStream::connect(server.addr).await.unwrap();
    stalled.write_all(b"GET / HTTP/1.1\r\nHo").await.unwrap();
    sleep(Duration::from_millis(50)).await;

    server.stop().await;

    // The sockets must hear about it, whatever the stalled connection does.
    assert_eq!(close_code(&mut client).await, Some(CloseCode::Away));
    drop(stalled);
}

#[tokio::test]
async fn a_close_from_the_client_is_answered() {
    let dir = TempDir::new().unwrap();
    let server = start_default(&dir).await;
    let mut client = server.client("doc").await;
    client.send(Message::Close(None)).await.unwrap();

    let answered = timeout(Duration::from_secs(5), async {
        loop {
            match client.next().await {
                Some(Ok(Message::Close(_))) => return true,
                Some(Ok(_)) => {}
                Some(Err(_)) | None => return false,
            }
        }
    })
    .await
    .expect("the socket must close");
    assert!(answered, "the close handshake must complete");
    server.stop().await;
}

#[tokio::test]
async fn a_text_frame_closes_the_socket() {
    let dir = TempDir::new().unwrap();
    let server = start_default(&dir).await;
    let mut client = server.client("doc").await;
    client.send(Message::text("hello")).await.unwrap();
    assert_eq!(close_code(&mut client).await, Some(CloseCode::Protocol));
    server.stop().await;
}

#[tokio::test]
async fn a_malformed_frame_closes_the_socket() {
    let dir = TempDir::new().unwrap();
    let server = start_default(&dir).await;
    let mut client = server.client("doc").await;
    client
        .send(Message::binary(vec![0xff, 0xff, 0xff]))
        .await
        .unwrap();
    assert_eq!(close_code(&mut client).await, Some(CloseCode::Protocol));
    server.stop().await;
}

#[tokio::test]
async fn a_frame_over_the_limit_closes_the_socket_with_1009() {
    let dir = TempDir::new().unwrap();
    let collab = CollabConfig {
        max_frame_bytes: 1024,
        ..collab()
    };
    let server = start(state(&dir, collab, SocketConfig::default(), WRITER).await).await;

    // Just past the limit, and far past it: both must end the same way.
    for size in [1025, 1024 * 1024] {
        let mut client = server.client("doc").await;
        client.send(Message::binary(vec![0; size])).await.unwrap();
        assert_eq!(
            close_code(&mut client).await,
            Some(CloseCode::Size),
            "{size}"
        );
    }
    server.stop().await;
}

#[tokio::test]
async fn sockets_past_the_limit_get_503_before_the_upgrade() {
    let dir = TempDir::new().unwrap();
    let collab = CollabConfig {
        max_sockets: 1,
        ..collab()
    };
    let server = start(state(&dir, collab, SocketConfig::default(), WRITER).await).await;
    let _client = server.client("doc").await;
    assert_eq!(
        refused_status(server.request("doc")).await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    server.stop().await;
}

#[tokio::test]
async fn only_the_public_origin_can_open_a_socket() {
    let dir = TempDir::new().unwrap();
    let server = start_default(&dir).await;

    let mut foreign = server.request("doc");
    foreign
        .headers_mut()
        .insert(header::ORIGIN, "https://evil.example".parse().unwrap());
    assert_eq!(refused_status(foreign).await, StatusCode::FORBIDDEN);

    // DNS rebinding: a hostile name pointed at this server, so Origin and Host agree.
    let mut rebound = server.request("doc");
    let headers = rebound.headers_mut();
    headers.insert(header::ORIGIN, "http://evil.example".parse().unwrap());
    headers.insert(header::HOST, "evil.example".parse().unwrap());
    assert_eq!(refused_status(rebound).await, StatusCode::FORBIDDEN);

    // Matching `Host` no longer means anything.
    let mut same_host = server.request("doc");
    let origin = format!("http://{}", server.addr);
    same_host
        .headers_mut()
        .insert(header::ORIGIN, origin.parse().unwrap());
    assert_eq!(refused_status(same_host).await, StatusCode::FORBIDDEN);

    let mut public = server.request("doc");
    public
        .headers_mut()
        .insert(header::ORIGIN, PUBLIC_ORIGIN.parse().unwrap());
    connect_async(public)
        .await
        .expect("the public origin is allowed");

    connect_async(server.request("doc"))
        .await
        .expect("a client outside a browser sends no origin");

    server.stop().await;
}

/// Opening a room can take long: what the role allowed before must not decide after.
#[tokio::test]
async fn a_rename_that_waits_for_its_room_checks_the_role_again() {
    let dir = TempDir::new().unwrap();
    let state = state(&dir, collab(), SocketConfig::default(), WRITER).await;
    let store = state.store.clone();
    let registry = state.registry.clone();
    store.hold_load.store(true, Ordering::SeqCst);
    let request = Request::patch("/api/v1/pages/doc")
        .header(header::COOKIE, format!("session={TOKEN}"))
        .header("x-csrf-token", csrf_token(TOKEN))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"title":"Too late"}"#))
        .unwrap();
    let rename = tokio::spawn(router::<_, _, Fixtures>(state).oneshot(request));

    store.load_entered.notified().await;
    store
        .inner
        .trash_page("doc", "user", now_ms())
        .await
        .unwrap();
    store.hold_load.store(false, Ordering::SeqCst);
    store.release_load.notify_one();

    let response = rename.await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    registry.shutdown().await.unwrap();
    assert!(store.inner.load("doc").await.unwrap().updates.is_empty());
}

#[tokio::test]
async fn a_refused_document_is_not_found() {
    let dir = TempDir::new().unwrap();
    let state = state(&dir, collab(), SocketConfig::default(), Fixed(None)).await;
    let server = start(state).await;
    assert_eq!(
        refused_status(server.request("doc")).await,
        StatusCode::NOT_FOUND
    );
    server.stop().await;
}

#[tokio::test]
async fn the_authorizer_sets_the_mode_of_the_socket() {
    let dir = TempDir::new().unwrap();
    let authorizer = Fixed(Some(SocketMode::ReadOnly));
    let server = start(state(&dir, collab(), SocketConfig::default(), authorizer).await).await;
    let mut client = server.client("doc").await;
    client
        .send(Message::binary(update_frame("refused")))
        .await
        .unwrap();

    let auth_frame = timeout(Duration::from_secs(5), async {
        loop {
            if let Message::Binary(frame) = client.next().await.unwrap().unwrap()
                && frame[0] == 2
            {
                return;
            }
        }
    })
    .await;
    assert!(auth_frame.is_ok(), "a read-only socket gets an Auth frame");
    server.stop().await;
}

#[tokio::test]
async fn the_server_pings_and_a_silent_client_is_closed() {
    let dir = TempDir::new().unwrap();
    let socket = SocketConfig {
        ping: Duration::from_millis(50),
        ping_timeout: Duration::from_millis(200),
        ..SocketConfig::default()
    };
    let server = start(state(&dir, collab(), socket, WRITER).await).await;
    let mut client = server.client("doc").await;

    // Not polling the client means no pong goes out, so the server hears silence.
    sleep(Duration::from_millis(500)).await;

    let mut pinged = false;
    let code = timeout(Duration::from_secs(5), async {
        loop {
            match client.next().await {
                Some(Ok(Message::Ping(_))) => pinged = true,
                Some(Ok(Message::Close(frame))) => return frame.map(|frame| frame.code),
                Some(Ok(_)) => {}
                Some(Err(_)) | None => return None,
            }
        }
    })
    .await
    .expect("the socket must close");
    assert!(pinged);
    assert_eq!(code, Some(CloseCode::from(4408)));
    server.stop().await;
}

#[tokio::test]
async fn a_client_that_answers_pings_stays_connected() {
    let dir = TempDir::new().unwrap();
    let socket = SocketConfig {
        ping: Duration::from_millis(20),
        ping_timeout: Duration::from_millis(100),
        ..SocketConfig::default()
    };
    let server = start(state(&dir, collab(), socket, WRITER).await).await;
    let mut client = server.client("doc").await;

    // Reading is what makes tungstenite answer the pings.
    let closed = timeout(Duration::from_millis(400), async {
        while let Some(Ok(message)) = client.next().await {
            if message.is_close() {
                return;
            }
        }
    })
    .await;
    assert!(closed.is_err(), "the socket must stay open");
    server.stop().await;
}
