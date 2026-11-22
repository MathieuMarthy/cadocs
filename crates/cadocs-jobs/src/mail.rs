//! One message to a user who asked for mail, telling of the notifications they have not read.

use std::sync::Arc;

use cadocs_mail::{Mail, Outbox};
use cadocs_store::{CommentStore, Job, MailDigest, NotificationKind, now_ms};
use serde_json::Value;

use crate::JobError;

/// At most this many notifications are listed: the message says how many more there are.
const LISTED: usize = 20;

/// Runs `MAIL_NOTIFICATIONS` jobs. With no relay (`outbox` is `None`) it does nothing.
pub struct MailNotifications<S> {
    store: Arc<S>,
    outbox: Option<Outbox>,
    origin: String,
}

impl<S> MailNotifications<S> {
    pub fn new(store: Arc<S>, outbox: Option<Outbox>, origin: String) -> Self {
        Self {
            store,
            outbox,
            origin,
        }
    }
}

impl<S: CommentStore> MailNotifications<S> {
    pub async fn run(&self, job: &Job) -> Result<(), JobError> {
        let Some(outbox) = &self.outbox else {
            return Ok(());
        };
        let payload: Value = serde_json::from_str(&job.payload)?;
        let user = payload["user"].as_str().ok_or("a mail job names no user")?;
        let Some(digest) = self.store.mail_digest(user).await? else {
            return Ok(());
        };
        let ids: Vec<String> = digest.items.iter().map(|item| item.id.clone()).collect();
        // A full outbox fails the job, which is retried later: nothing is marked as told.
        if !outbox.queue(message(&digest, &self.origin)) {
            return Err("the outbox is full".into());
        }
        self.store.mark_mailed(user, &ids, now_ms()).await?;
        Ok(())
    }
}

fn message(digest: &MailDigest, origin: &str) -> Mail {
    let mut text = format!("Hello {},\n\n", digest.name);
    let mut seen: Vec<String> = Vec::new();
    for item in &digest.items {
        let who = item.actor.as_ref().map_or("Someone", |actor| &actor.name);
        let what = match item.kind {
            NotificationKind::Mention => "mentioned you in",
            NotificationKind::Reply => "replied to you on",
            NotificationKind::Comment => "commented on",
        };
        let title = match item.page_title.as_str() {
            "" => "an untitled page",
            title => title,
        };
        // Two comments on one page by one person read as one line.
        let line = format!("{who} {what} {title}\n{origin}/?page={}\n\n", item.page_id);
        if !seen.contains(&line) {
            seen.push(line);
        }
    }
    text.extend(seen.iter().take(LISTED).map(String::as_str));
    if seen.len() > LISTED {
        text.push_str(&format!("And {} more.\n\n", seen.len() - LISTED));
    }
    text.push_str("You get this because notifications by mail are on in your account.\n");
    let count = digest.items.len();
    Mail {
        to: digest.email.clone(),
        subject: match count {
            1 => "1 new notification on cadocs".to_owned(),
            _ => format!("{count} new notifications on cadocs"),
        },
        text,
    }
}
