//! Simulated editors: each one speaks y-sync over its own socket.

use std::time::Duration;

use anyhow::{Context, anyhow, bail, ensure};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::{Instant, interval_at, sleep, timeout};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::http::header::COOKIE;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use yrs::sync::{Awareness, Message, SyncMessage};
use yrs::types::xml::XmlOut;
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, Options, ReadTxn, Text, Transact, Update, XmlFragment, XmlFragmentRef};

use crate::document::{BODY, client_id};
use crate::rng::Rng;
use crate::seed::{WORKSPACE, doc_id, session_token};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
const TYPED: &[&str] = &["a", "e", "the ", "and ", "x", " "];

#[derive(Debug, Clone)]
pub struct LoadConfig {
    /// Base address, as `ws://127.0.0.1:3000`.
    pub url: String,
    pub docs: usize,
    pub clients_per_doc: usize,
    /// Client `i` uses the session of seeded user `i % users`.
    pub users: usize,
    /// Each client also holds an events socket, as the front end does beside its page.
    pub events: bool,
    pub updates_per_sec: u32,
    pub awareness_per_sec: u32,
    pub duration: Duration,
    /// Time clients stay connected after the load stops, before `then` runs.
    pub hold: Duration,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LoadReport {
    pub clients: usize,
    pub updates_sent: u64,
    pub awareness_sent: u64,
    pub updates_received: u64,
    /// Events sockets still open when the load ends.
    pub events: usize,
}

impl LoadReport {
    /// Refuses a run that did not do what the scenario asks. A client may lose one tick to its
    /// random phase, and 1% covers updates still in flight.
    pub fn check(&self, config: &LoadConfig) -> anyhow::Result<()> {
        let clients = (config.docs * config.clients_per_doc) as u64;
        let secs = config.duration.as_secs_f64();
        let enough = |actual: u64, rate: u32| {
            let expected = clients as f64 * f64::from(rate) * secs;
            actual as f64 >= expected * 0.99 - clients as f64
        };
        let others = config.clients_per_doc as u64 - 1;
        ensure!(self.clients as u64 == clients, "missing clients: {self:?}");
        if config.events {
            ensure!(
                self.events == self.clients,
                "missing events sockets: {self:?}"
            );
        }
        ensure!(
            enough(self.updates_sent, config.updates_per_sec),
            "too few updates: {self:?}"
        );
        ensure!(
            enough(self.awareness_sent, config.awareness_per_sec),
            "too few awareness updates: {self:?}"
        );
        ensure!(
            self.updates_received as f64 >= (self.updates_sent * others) as f64 * 0.99,
            "updates did not reach the other clients: {self:?}"
        );
        Ok(())
    }

    fn add(&mut self, other: LoadReport) {
        self.clients += other.clients;
        self.updates_sent += other.updates_sent;
        self.awareness_sent += other.awareness_sent;
        self.updates_received += other.updates_received;
        self.events += other.events;
    }
}

/// An upgrade on the session the seed opened for `user`.
fn upgrade(url: &str, path: &str, user: usize) -> anyhow::Result<Request> {
    let mut request = format!("{url}{path}").into_client_request()?;
    let cookie = format!("session={}", session_token(user)).parse()?;
    request.headers_mut().insert(COOKIE, cookie);
    Ok(request)
}

fn page_request(url: &str, doc: &str, user: usize) -> anyhow::Result<Request> {
    upgrade(url, &format!("/ws/pages/{doc}"), user)
}

/// Apart from the seeds of the documents, or a client would continue its author's clock.
const CLIENT_SEEDS: u64 = 1 << 32;

pub fn load_client_id(index: u64) -> yrs::ClientID {
    client_id(&mut Rng::new(CLIENT_SEEDS + index))
}

pub(crate) struct Client {
    name: String,
    socket: Socket,
    events: Option<Socket>,
    doc: Doc,
    body: XmlFragmentRef,
    awareness: Awareness,
    rng: Rng,
    report: LoadReport,
}

fn sync(message: SyncMessage) -> WsMessage {
    WsMessage::binary(Message::Sync(message).encode_v1())
}

impl Client {
    /// Connects and finishes the handshake of `docs/API.md`, so the document is fully loaded.
    pub(crate) async fn connect(
        url: &str,
        doc: usize,
        index: u64,
        user: usize,
    ) -> anyhow::Result<Self> {
        let name = format!("{}#{index}", doc_id(doc));
        let (socket, _) = connect_async(page_request(url, &doc_id(doc), user)?)
            .await
            .with_context(|| format!("{name} cannot connect"))?;
        let doc = Doc::with_options(Options {
            client_id: load_client_id(index),
            ..Options::default()
        });
        let mut client = Self {
            name,
            socket,
            events: None,
            body: doc.get_or_insert_xml_fragment(BODY),
            awareness: Awareness::new(doc.clone()),
            doc,
            rng: Rng::new(index),
            report: LoadReport {
                clients: 1,
                ..LoadReport::default()
            },
        };
        let state_vector = client.doc.transact().state_vector();
        client
            .socket
            .send(sync(SyncMessage::SyncStep1(state_vector)))
            .await?;
        timeout(HANDSHAKE_TIMEOUT, client.finish_handshake())
            .await
            .with_context(|| format!("{}: no handshake", client.name))??;
        Ok(client)
    }

    async fn finish_handshake(&mut self) -> anyhow::Result<()> {
        let (mut loaded, mut answered) = (false, false);
        while !(loaded && answered) {
            let frame = self.next_frame().await?;
            match Message::decode_v1(&frame)? {
                Message::Sync(SyncMessage::SyncStep1(_)) => answered = true,
                Message::Sync(SyncMessage::SyncStep2(_)) => loaded = true,
                _ => {}
            }
            self.handle(&frame).await?;
        }
        Ok(())
    }

    /// Opens the events socket of the seeded workspace.
    async fn listen(&mut self, url: &str, user: usize) -> anyhow::Result<()> {
        let path = format!("/ws/events?workspace={WORKSPACE}");
        let (socket, _) = connect_async(upgrade(url, &path, user)?)
            .await
            .with_context(|| format!("{}: no events socket", self.name))?;
        self.events = Some(socket);
        Ok(())
    }

    /// Reads the events socket, which also answers its pings. Pending forever without one.
    async fn next_event(events: &mut Option<Socket>, name: &str) -> anyhow::Result<()> {
        let Some(socket) = events else {
            return std::future::pending().await;
        };
        match socket.next().await {
            Some(Ok(WsMessage::Close(close))) => bail!("{name}: events closed: {close:?}"),
            Some(Ok(_)) => Ok(()),
            Some(Err(error)) => bail!("{name}: events: {error}"),
            None => bail!("{name}: the events socket ended"),
        }
    }

    /// Closes the page socket. The run has its measure by then, so a failing close changes
    /// nothing.
    pub(crate) async fn close(mut self) {
        let _ = self.socket.close(None).await;
    }

    async fn next_frame(&mut self) -> anyhow::Result<Vec<u8>> {
        loop {
            match self.socket.next().await {
                Some(Ok(WsMessage::Binary(frame))) => return Ok(frame.into()),
                Some(Ok(WsMessage::Close(close))) => bail!("{}: closed: {close:?}", self.name),
                Some(Ok(_)) => {}
                Some(Err(error)) => bail!("{}: {error}", self.name),
                None => bail!("{}: the socket ended", self.name),
            }
        }
    }

    /// What y-websocket does with a frame: answer SyncStep1, apply updates, and here skip the
    /// awareness of others, which only feeds a client's UI.
    async fn handle(&mut self, frame: &[u8]) -> anyhow::Result<()> {
        match Message::decode_v1(frame)? {
            Message::Sync(SyncMessage::SyncStep1(state_vector)) => {
                let missing = self.doc.transact().encode_state_as_update_v1(&state_vector);
                self.socket
                    .send(sync(SyncMessage::SyncStep2(missing)))
                    .await?;
            }
            Message::Sync(SyncMessage::SyncStep2(update)) => self.apply(&update)?,
            Message::Sync(SyncMessage::Update(update)) => {
                self.apply(&update)?;
                self.report.updates_received += 1;
            }
            _ => {}
        }
        Ok(())
    }

    fn apply(&self, update: &[u8]) -> anyhow::Result<()> {
        self.doc
            .transact_mut()
            .apply_update(Update::decode_v1(update)?)?;
        Ok(())
    }

    /// Types a few characters somewhere in the page.
    fn edit(&mut self) -> anyhow::Result<Vec<u8>> {
        let mut txn = self.doc.transact_mut();
        let paragraphs = self.body.len(&txn) as usize;
        ensure!(paragraphs > 0, "{}: the page is empty", self.name);
        let index = self.rng.below(paragraphs) as u32;
        let Some(XmlOut::Element(paragraph)) = self.body.get(&txn, index) else {
            bail!("{}: not a paragraph", self.name);
        };
        let Some(XmlOut::Text(text)) = paragraph.get(&txn, 0) else {
            bail!("{}: a paragraph without text", self.name);
        };
        let at = self.rng.below(text.len(&txn) as usize + 1) as u32;
        text.insert(&mut txn, at, TYPED[self.rng.below(TYPED.len())]);
        Ok(txn.encode_update_v1())
    }

    /// The state a TipTap client publishes: its name, color and a cursor.
    fn awareness_frame(&mut self) -> anyhow::Result<WsMessage> {
        let position = format!(
            r#"{{"type":null,"tname":null,"item":{{"client":{},"clock":{}}},"assoc":0}}"#,
            self.doc.client_id(),
            self.rng.below(20_000),
        );
        self.awareness.set_local_state_raw(format!(
            r#"{{"user":{{"name":"{}","slot":{}}},"cursor":{{"anchor":{position},"head":{position}}}}}"#,
            self.name,
            1 + self.rng.below(3),
        ));
        let update = self.awareness.update()?;
        Ok(WsMessage::binary(Message::Awareness(update).encode_v1()))
    }

    /// Sends for `duration`, then only listens until `stop`. Returning early is an error.
    async fn run(
        mut self,
        config: LoadConfig,
        mut stop: watch::Receiver<bool>,
    ) -> anyhow::Result<LoadReport> {
        let edit_period = Duration::from_secs(1) / config.updates_per_sec;
        let awareness_period = Duration::from_secs(1) / config.awareness_per_sec;
        // A random phase, so the clients do not all send in the same millisecond.
        let now = Instant::now();
        let phase = |rng: &mut Rng, period: Duration| {
            now + period.mul_f64(rng.below(1_000) as f64 / 1_000.0)
        };
        let mut edits = interval_at(phase(&mut self.rng, edit_period), edit_period);
        let mut presence = interval_at(phase(&mut self.rng, awareness_period), awareness_period);
        let end = sleep(config.duration);
        tokio::pin!(end);
        // A zero duration sends nothing, not even a first tick: its pages may be empty.
        let mut active = !config.duration.is_zero();
        // Apart from `self`, which the page socket's branch borrows whole.
        let mut events = self.events.take();
        let name = self.name.clone();

        loop {
            tokio::select! {
                _ = stop.changed() => break,
                () = &mut end, if active => active = false,
                _ = edits.tick(), if active => {
                    let update = self.edit()?;
                    self.socket.send(sync(SyncMessage::Update(update))).await?;
                    self.report.updates_sent += 1;
                }
                _ = presence.tick(), if active => {
                    let frame = self.awareness_frame()?;
                    self.socket.send(frame).await?;
                    self.report.awareness_sent += 1;
                }
                frame = self.next_frame() => self.handle(&frame?).await?,
                event = Self::next_event(&mut events, &name) => event?,
            }
        }
        // Past this point the run has its measure: a failing close changes nothing.
        let _ = self.socket.close(None).await;
        if let Some(mut events) = events {
            self.report.events = 1;
            let _ = events.close(None).await;
        }
        Ok(self.report)
    }
}

fn lost_client(
    joined: Option<Result<anyhow::Result<LoadReport>, tokio::task::JoinError>>,
) -> anyhow::Error {
    match joined {
        Some(Ok(Err(error))) => error.context("a client stopped during the run"),
        Some(Err(error)) => anyhow!(error).context("a client crashed"),
        _ => anyhow!("a client stopped during the run"),
    }
}

/// Runs the load. `then` runs after the hold, with every client still connected. Any client that
/// loses its socket fails the run: a measure with fewer clients than the scenario means nothing.
pub async fn run<F: Future<Output = anyhow::Result<()>>>(
    config: &LoadConfig,
    then: F,
) -> anyhow::Result<LoadReport> {
    ensure!(
        config.updates_per_sec > 0 && config.awareness_per_sec > 0,
        "rates must be above 0"
    );
    ensure!(config.users > 0, "the load needs a user");
    let mut connecting = JoinSet::new();
    for doc in 0..config.docs {
        for slot in 0..config.clients_per_doc {
            let url = config.url.clone();
            let index = doc * config.clients_per_doc + slot;
            let user = index % config.users;
            let events = config.events;
            connecting.spawn(async move {
                let mut client = Client::connect(&url, doc, index as u64, user).await?;
                if events {
                    client.listen(&url, user).await?;
                }
                anyhow::Ok(client)
            });
        }
    }

    // Everyone first: an edit sent before a peer joins would reach it in its handshake instead.
    let mut connected = Vec::new();
    while let Some(client) = connecting.join_next().await {
        connected.push(client??);
    }

    let (stop, stopped) = watch::channel(false);
    let mut clients = JoinSet::new();
    for client in connected {
        clients.spawn(client.run(config.clone(), stopped.clone()));
    }

    tokio::select! {
        () = sleep(config.duration + config.hold) => {}
        joined = clients.join_next() => return Err(lost_client(joined)),
    }
    tokio::select! {
        result = then => result?,
        joined = clients.join_next() => return Err(lost_client(joined)),
    }

    stop.send(true)?;
    let mut report = LoadReport::default();
    while let Some(client) = clients.join_next().await {
        report.add(client??);
    }
    report.check(config)?;
    Ok(report)
}

/// The awareness states a new client of `doc` receives on connecting.
pub async fn peers(url: &str, doc: &str) -> anyhow::Result<usize> {
    let (mut socket, _) = connect_async(page_request(url, doc, 0)?).await?;
    let count = timeout(HANDSHAKE_TIMEOUT, async {
        while let Some(frame) = socket.next().await {
            if let WsMessage::Binary(frame) = frame?
                && let Message::Awareness(update) = Message::decode_v1(&frame)?
            {
                return Ok(update.clients.len());
            }
        }
        bail!("the socket ended before any awareness state")
    })
    .await
    .context("no awareness state")??;
    let _ = socket.close(None).await;
    Ok(count)
}
