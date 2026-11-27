//! One message to a user who asked for mail, telling of the notifications they have not read.

use std::sync::Arc;

use cadocs_mail::{Lang, Mail, Outbox, Text};
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
    let lang = Lang::of(digest.language.as_deref());
    let mut text = Text::Hello.fill(lang, &[("name", &digest.name)]);
    let mut seen: Vec<String> = Vec::new();
    for item in &digest.items {
        let who = item
            .actor
            .as_ref()
            .map_or(Text::Someone.get(lang), |actor| &actor.name);
        let what = match item.kind {
            NotificationKind::Mention => Text::MentionLine,
            NotificationKind::Reply => Text::ReplyLine,
            NotificationKind::Comment => Text::CommentLine,
        };
        let title = match item.page_title.as_str() {
            "" => Text::UntitledPage.get(lang),
            title => title,
        };
        // Two comments on one page by one person read as one line.
        let mut line = what.fill(lang, &[("who", who), ("title", title)]);
        line.push_str(&format!("{origin}/?page={}\n\n", item.page_id));
        if !seen.contains(&line) {
            seen.push(line);
        }
    }
    text.extend(seen.iter().take(LISTED).map(String::as_str));
    if seen.len() > LISTED {
        let more = (seen.len() - LISTED).to_string();
        text.push_str(&Text::More.fill(lang, &[("count", &more)]));
    }
    text.push_str(Text::NotificationsFooter.get(lang));
    let count = digest.items.len();
    Mail {
        to: digest.email.clone(),
        subject: match count {
            1 => Text::NotificationSubject.get(lang).to_owned(),
            _ => Text::NotificationsSubject.fill(lang, &[("count", &count.to_string())]),
        },
        text,
    }
}
