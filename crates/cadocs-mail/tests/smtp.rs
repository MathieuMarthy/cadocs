//! The SMTP mailer against a relay that keeps what it is sent.

use std::time::Duration;

use cadocs_mail::fake::{Behavior, FakeRelay};
use cadocs_mail::{Mail, MailError, Mailer, Security, SmtpConfig, SmtpMailer};

fn config(relay: &FakeRelay) -> SmtpConfig {
    SmtpConfig {
        host: "127.0.0.1".to_owned(),
        port: relay.port(),
        security: Security::None,
        credentials: None,
        from: "Team wiki <wiki@example.org>".to_owned(),
    }
}

fn mail() -> Mail {
    Mail {
        to: "ada@example.net".to_owned(),
        subject: "Your code".to_owned(),
        text: "Your code is 123456.\n.a line that starts with a dot\n".to_owned(),
    }
}

fn header<'a>(data: &'a str, name: &str) -> Option<&'a str> {
    let headers = data.split("\r\n\r\n").next().unwrap();
    headers
        .split("\r\n")
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(": "))
}

#[tokio::test]
async fn a_message_reaches_the_relay() {
    let relay = FakeRelay::start(Behavior::default()).await;
    let mailer = SmtpMailer::new(config(&relay)).unwrap();

    mailer.send(&mail()).await.unwrap();

    let messages = relay.messages();
    assert_eq!(messages.len(), 1);
    let message = &messages[0];
    assert!(
        message.from.starts_with("<wiki@example.org>"),
        "{message:?}"
    );
    assert!(message.to.starts_with("<ada@example.net>"), "{message:?}");
    let data = &message.data;
    assert_eq!(header(data, "To"), Some("ada@example.net"));
    assert_eq!(header(data, "Subject"), Some("Your code"));
    assert!(header(data, "From").unwrap().contains("<wiki@example.org>"));
    assert!(header(data, "Date").is_some(), "{data}");
    assert!(
        header(data, "Content-Type")
            .unwrap()
            .starts_with("text/plain"),
        "{data}"
    );
    // A relay scores a message whose id names no real host as spam.
    let id = header(data, "Message-ID").unwrap();
    assert!(id.ends_with("@example.org>"), "{id}");
    let body = data.split_once("\r\n\r\n").unwrap().1;
    assert_eq!(
        body,
        "Your code is 123456.\r\n.a line that starts with a dot\r\n"
    );
    // The relay is greeted with a name, not with the default `[127.0.0.1]`.
    assert_eq!(relay.commands()[0], "EHLO example.org");
}

#[tokio::test]
async fn two_messages_have_two_ids() {
    let relay = FakeRelay::start(Behavior::default()).await;
    let mailer = SmtpMailer::new(config(&relay)).unwrap();

    mailer.send(&mail()).await.unwrap();
    mailer.send(&mail()).await.unwrap();

    let messages = relay.messages();
    assert_ne!(
        header(&messages[0].data, "Message-ID"),
        header(&messages[1].data, "Message-ID")
    );
}

#[tokio::test]
async fn credentials_are_sent_when_set() {
    let relay = FakeRelay::start(Behavior {
        auth: true,
        ..Behavior::default()
    })
    .await;
    let mailer = SmtpMailer::new(SmtpConfig {
        credentials: Some(("ada".to_owned(), "s3cret".to_owned())),
        ..config(&relay)
    })
    .unwrap();

    mailer.send(&mail()).await.unwrap();

    assert_eq!(relay.messages().len(), 1);
    // `\0ada\0s3cret` in base64.
    assert!(
        relay
            .commands()
            .contains(&"AUTH PLAIN AGFkYQBzM2NyZXQ=".to_owned()),
        "{:?}",
        relay.commands()
    );
}

#[tokio::test]
async fn nothing_is_sent_in_clear_when_tls_is_asked_for() {
    for security in [Security::StartTls, Security::Tls] {
        // This relay offers no STARTTLS and speaks no TLS.
        let relay = FakeRelay::start(Behavior {
            auth: true,
            ..Behavior::default()
        })
        .await;
        let mailer = SmtpMailer::new(SmtpConfig {
            security,
            credentials: Some(("ada".to_owned(), "s3cret".to_owned())),
            ..config(&relay)
        })
        .unwrap();

        let error = mailer.send(&mail()).await.unwrap_err();

        assert!(matches!(error, MailError::Transport(_)), "{error:?}");
        assert_eq!(relay.messages(), []);
        let commands = relay.commands();
        assert!(
            !commands.iter().any(|line| line.starts_with("AUTH")),
            "{commands:?}"
        );
        assert!(
            !commands.iter().any(|line| line.starts_with("MAIL")),
            "{commands:?}"
        );
    }
}

#[tokio::test]
async fn a_refused_recipient_is_told_apart_from_a_relay_that_is_down() {
    let relay = FakeRelay::start(Behavior {
        refuse_recipients: true,
        ..Behavior::default()
    })
    .await;
    let mailer = SmtpMailer::new(config(&relay)).unwrap();

    let error = mailer.send(&mail()).await.unwrap_err();
    assert!(matches!(error, MailError::Rejected(_)), "{error:?}");
    assert!(error.to_string().contains("no such user"), "{error}");

    let down = SmtpMailer::new(SmtpConfig {
        // Nothing listens there: the relay took the only port this test opened.
        port: 1,
        ..config(&relay)
    })
    .unwrap();
    let error = down.send(&mail()).await.unwrap_err();
    assert!(matches!(error, MailError::Transport(_)), "{error:?}");
}

#[tokio::test]
async fn a_subject_or_an_address_cannot_add_a_header() {
    let relay = FakeRelay::start(Behavior::default()).await;
    let mailer = SmtpMailer::new(config(&relay)).unwrap();

    let error = mailer
        .send(&Mail {
            to: "ada@example.net\r\nBcc: eve@example.com".to_owned(),
            ..mail()
        })
        .await
        .unwrap_err();
    assert!(matches!(error, MailError::Address(_)), "{error:?}");
    assert_eq!(relay.messages(), []);

    // Whether it is refused or encoded, no line of the message may begin with the header.
    let sent = mailer
        .send(&Mail {
            subject: "Hello\r\nBcc: eve@example.com".to_owned(),
            ..mail()
        })
        .await;
    for message in relay.messages() {
        assert!(!message.data.contains("\nBcc:"), "{}", message.data);
        assert!(!message.to.contains("eve"), "{message:?}");
    }
    assert_eq!(relay.messages().len(), usize::from(sent.is_ok()));
}

#[tokio::test]
async fn text_outside_ascii_keeps_its_charset() {
    let relay = FakeRelay::start(Behavior::default()).await;
    let mailer = SmtpMailer::new(config(&relay)).unwrap();

    mailer
        .send(&Mail {
            subject: "Votre code d\u{2019}accès".to_owned(),
            text: "Bonjour Zoé\n".to_owned(),
            ..mail()
        })
        .await
        .unwrap();

    let data = &relay.messages()[0].data;
    assert!(data.is_ascii(), "{data}");
    let content_type = header(data, "Content-Type").unwrap().to_ascii_lowercase();
    assert!(content_type.contains("charset=utf-8"), "{content_type}");
    assert!(header(data, "Subject").unwrap().contains("=?utf-8?"));
}

#[tokio::test]
async fn a_relay_that_says_nothing_does_not_hold_the_sender() {
    let relay = FakeRelay::start(Behavior {
        silent: true,
        ..Behavior::default()
    })
    .await;
    let mailer = SmtpMailer::new(config(&relay))
        .unwrap()
        .with_timeout(Duration::from_millis(200));

    let error = tokio::time::timeout(Duration::from_secs(5), mailer.send(&mail()))
        .await
        .expect("the mailer gives up by itself")
        .unwrap_err();
    assert!(matches!(error, MailError::Transport(_)), "{error:?}");
}

#[tokio::test]
async fn the_check_tells_whether_the_relay_answers() {
    let relay = FakeRelay::start(Behavior::default()).await;
    SmtpMailer::new(config(&relay))
        .unwrap()
        .check()
        .await
        .unwrap();
    assert_eq!(relay.messages(), []);

    let down = SmtpMailer::new(SmtpConfig {
        port: 1,
        ..config(&relay)
    })
    .unwrap();
    assert!(down.check().await.is_err());
}

#[test]
fn a_sender_that_is_no_address_is_refused() {
    let relay_free = |from: &str| {
        SmtpMailer::new(SmtpConfig {
            host: "127.0.0.1".to_owned(),
            port: 25,
            security: Security::None,
            credentials: None,
            from: from.to_owned(),
        })
    };
    assert!(relay_free("wiki@example.org").is_ok());
    assert!(relay_free("Team wiki <wiki@example.org>").is_ok());
    // A domain outside ASCII would go as it is into `EHLO` and the message id.
    let idn = "wiki@ex\u{e4}mple.org";
    for from in ["", "wiki", "Team wiki", "a@example.org, b@example.org", idn] {
        let error = relay_free(from).err().expect(from);
        assert!(matches!(error, MailError::Address(_)), "{from}: {error:?}");
    }
}

#[test]
fn the_password_stays_out_of_debug_output() {
    let config = SmtpConfig {
        host: "127.0.0.1".to_owned(),
        port: 25,
        security: Security::None,
        credentials: Some(("ada".to_owned(), "s3cret".to_owned())),
        from: "wiki@example.org".to_owned(),
    };
    let shown = format!("{config:?}");
    assert!(shown.contains("ada"), "{shown}");
    assert!(!shown.contains("s3cret"), "{shown}");
}
