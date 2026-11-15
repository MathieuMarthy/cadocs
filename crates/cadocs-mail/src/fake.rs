//! A relay for tests: it speaks enough SMTP for one message per connection, in plain text, and
//! keeps what it is sent.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// How the relay behaves.
#[derive(Debug, Clone, Copy, Default)]
pub struct Behavior {
    /// Offers `AUTH PLAIN`.
    pub auth: bool,
    /// Answers `550` to every recipient.
    pub refuse_recipients: bool,
    /// Accepts connections and never says a word.
    pub silent: bool,
}

/// A message the relay accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Received {
    /// The argument of `MAIL FROM`, as sent.
    pub from: String,
    /// The argument of `RCPT TO`, as sent.
    pub to: String,
    /// Headers and body, with CRLF line ends.
    pub data: String,
}

#[derive(Debug, Default)]
struct Seen {
    messages: Vec<Received>,
    commands: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FakeRelay {
    address: SocketAddr,
    seen: Arc<Mutex<Seen>>,
}

impl FakeRelay {
    /// Listens on a free local port until the runtime ends.
    pub async fn start(behavior: Behavior) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
        let address = listener.local_addr().expect("a local address");
        let seen = Arc::new(Mutex::new(Seen::default()));
        let shared = seen.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(talk(stream, behavior, shared.clone()));
            }
        });
        Self { address, seen }
    }

    pub fn port(&self) -> u16 {
        self.address.port()
    }

    pub fn messages(&self) -> Vec<Received> {
        self.seen.lock().unwrap().messages.clone()
    }

    /// Every command line received, in order, without the content of messages.
    pub fn commands(&self) -> Vec<String> {
        self.seen.lock().unwrap().commands.clone()
    }
}

async fn talk(stream: TcpStream, behavior: Behavior, seen: Arc<Mutex<Seen>>) {
    if behavior.silent {
        // Held open, so the client waits on the greeting.
        let mut stream = BufReader::new(stream);
        let _ = stream.read_line(&mut String::new()).await;
        return;
    }
    // A client that leaves early ends the conversation, which is not the relay's failure.
    let _ = converse(stream, behavior, seen).await;
}

async fn converse(
    stream: TcpStream,
    behavior: Behavior,
    seen: Arc<Mutex<Seen>>,
) -> std::io::Result<()> {
    let mut stream = BufReader::new(stream);
    stream.get_mut().write_all(b"220 fake ESMTP\r\n").await?;
    let (mut from, mut to) = (String::new(), String::new());
    loop {
        let mut line = String::new();
        if stream.read_line(&mut line).await? == 0 {
            return Ok(());
        }
        let line = line.trim_end().to_owned();
        seen.lock().unwrap().commands.push(line.clone());
        let upper = line.to_ascii_uppercase();
        let answer: &str = if upper.starts_with("EHLO") {
            if behavior.auth {
                "250-fake\r\n250 AUTH PLAIN\r\n"
            } else {
                "250 fake\r\n"
            }
        } else if upper.starts_with("AUTH") {
            "235 authenticated\r\n"
        } else if upper.starts_with("MAIL FROM:") {
            from = line["MAIL FROM:".len()..].to_owned();
            "250 ok\r\n"
        } else if upper.starts_with("RCPT TO:") {
            if behavior.refuse_recipients {
                "550 no such user\r\n"
            } else {
                to = line["RCPT TO:".len()..].to_owned();
                "250 ok\r\n"
            }
        } else if upper == "DATA" {
            stream.get_mut().write_all(b"354 go on\r\n").await?;
            let mut data = String::new();
            loop {
                let mut line = String::new();
                if stream.read_line(&mut line).await? == 0 {
                    return Ok(());
                }
                if line == ".\r\n" {
                    break;
                }
                // Dot stuffing: a line that starts with a dot arrives with one more.
                data.push_str(line.strip_prefix('.').unwrap_or(&line));
            }
            seen.lock().unwrap().messages.push(Received {
                from: from.clone(),
                to: to.clone(),
                data,
            });
            "250 queued\r\n"
        } else if upper == "NOOP" {
            "250 ok\r\n"
        } else if upper == "QUIT" {
            stream.get_mut().write_all(b"221 bye\r\n").await?;
            return Ok(());
        } else {
            "502 not implemented\r\n"
        };
        stream.get_mut().write_all(answer.as_bytes()).await?;
    }
}
