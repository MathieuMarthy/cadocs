//! Mail through the operator's relay (ADR 0026). `Mailer` is what the server holds, so tests swap
//! in one that records; `SmtpMailer` is the only real one.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use lettre::message::Mailbox;
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::transport::smtp::extension::ClientId;
use lettre::{Address, AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

#[cfg(feature = "fake")]
pub mod fake;

/// What lettre gets as its timeout, for a single step with the relay.
const IO_TIMEOUT: Duration = Duration::from_secs(15);
/// A whole message may take this many times the timeout above, and no longer.
const EXCHANGES: u32 = 4;

/// A plain text message to one address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mail {
    pub to: String,
    pub subject: String,
    pub text: String,
}

#[derive(Debug, thiserror::Error)]
pub enum MailError {
    /// Not an address a message can go to or come from.
    #[error("invalid address: {0}")]
    Address(String),
    /// The relay answered, and refused: trying again as it is will not help.
    #[error("the relay refused: {0}")]
    Rejected(String),
    /// The relay could not be reached, could not be trusted, or failed for now.
    #[error("the relay failed: {0}")]
    Transport(String),
}

pub type Sending<'a> = Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>>;

/// Object safe, so the server keeps an `Arc<dyn Mailer>`.
pub trait Mailer: Send + Sync + 'static {
    /// Hands the message to the relay. It returns once the relay took it or refused it.
    fn send<'a>(&'a self, mail: &'a Mail) -> Sending<'a>;
}

/// How the connection to the relay is protected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    /// Plain at first, then `STARTTLS`, which is never skipped.
    StartTls,
    /// TLS from the first byte.
    Tls,
    /// Plain text, for a relay on the same host or a private network.
    None,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub security: Security,
    /// A user name and a password.
    pub credentials: Option<(String, String)>,
    /// `wiki@example.org` or `Team wiki <wiki@example.org>`.
    pub from: String,
}

impl fmt::Debug for SmtpConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SmtpConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("security", &self.security)
            .field("user", &self.credentials.as_ref().map(|(user, _)| user))
            .field("from", &self.from)
            .finish_non_exhaustive()
    }
}

/// Sends each message on its own connection: nothing stays open or allocated between two.
pub struct SmtpMailer {
    config: SmtpConfig,
    from: Mailbox,
    timeout: Duration,
}

impl SmtpMailer {
    pub fn new(config: SmtpConfig) -> Result<Self, MailError> {
        let from: Mailbox = config
            .from
            .parse()
            .map_err(|_| MailError::Address(config.from.clone()))?;
        // The domain goes as it is into `EHLO` and the message id, which only take ASCII.
        if !from.email.domain().is_ascii() {
            return Err(MailError::Address(config.from.clone()));
        }
        Ok(Self {
            config,
            from,
            timeout: IO_TIMEOUT,
        })
    }

    /// For tests: a shorter timeout, a quarter of what a whole message may take.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Whether the relay answers, accepts the security asked for and, if any, the credentials.
    pub async fn check(&self) -> Result<(), MailError> {
        let transport = self.transport()?;
        match self.bounded(transport.test_connection()).await? {
            true => Ok(()),
            false => Err(MailError::Transport("no answer to NOOP".to_owned())),
        }
    }

    fn transport(&self) -> Result<AsyncSmtpTransport<Tokio1Executor>, MailError> {
        let SmtpConfig { host, port, .. } = &self.config;
        let parameters =
            || TlsParameters::new(host.clone()).map_err(|error| transport_error(&error));
        let tls = match self.config.security {
            Security::StartTls => Tls::Required(parameters()?),
            Security::Tls => Tls::Wrapper(parameters()?),
            Security::None => Tls::None,
        };
        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host.as_str())
            .port(*port)
            .tls(tls)
            .timeout(Some(self.timeout))
            // lettre would greet with `[127.0.0.1]`, which some relays refuse.
            .hello_name(ClientId::Domain(self.from.email.domain().to_owned()));
        if let Some((user, password)) = &self.config.credentials {
            builder = builder.credentials(Credentials::new(user.clone(), password.clone()));
        }
        Ok(builder.build())
    }

    fn message(&self, mail: &Mail) -> Result<Message, MailError> {
        let to: Address = mail
            .to
            .parse()
            .map_err(|_| MailError::Address(mail.to.clone()))?;
        let domain = self.from.email.domain();
        Message::builder()
            .from(self.from.clone())
            .to(Mailbox::new(None, to))
            .subject(mail.subject.as_str())
            .message_id(Some(format!("<{}@{domain}>", random_id()?)))
            .header(ContentType::TEXT_PLAIN)
            .body(mail.text.clone())
            .map_err(|error| MailError::Rejected(error.to_string()))
    }

    /// The timeout given to lettre bounds single steps at best, never a whole conversation.
    async fn bounded<T>(
        &self,
        work: impl Future<Output = Result<T, lettre::transport::smtp::Error>>,
    ) -> Result<T, MailError> {
        match tokio::time::timeout(self.timeout * EXCHANGES, work).await {
            Ok(Ok(done)) => Ok(done),
            Ok(Err(error)) if error.is_permanent() => Err(MailError::Rejected(describe(&error))),
            Ok(Err(error)) => Err(transport_error(&error)),
            Err(_) => Err(MailError::Transport("the relay took too long".to_owned())),
        }
    }
}

impl Mailer for SmtpMailer {
    fn send<'a>(&'a self, mail: &'a Mail) -> Sending<'a> {
        Box::pin(async move {
            let message = self.message(mail)?;
            let transport = self.transport()?;
            self.bounded(transport.send(message)).await.map(|_| ())
        })
    }
}

fn transport_error(error: &lettre::transport::smtp::Error) -> MailError {
    MailError::Transport(describe(error))
}

/// The error and what caused it: lettre keeps the relay's words or the TLS failure in the source.
fn describe(error: &lettre::transport::smtp::Error) -> String {
    let mut text = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

fn random_id() -> Result<String, MailError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|error| MailError::Transport(format!("no random source: {error}")))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
