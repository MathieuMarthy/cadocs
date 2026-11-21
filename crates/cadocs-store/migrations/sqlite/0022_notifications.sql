-- In-app notifications (step 41): one row per user told of one comment or body mention.
-- `kind` is `mention`, `reply` (to the user's comment) or `comment` (on the user's page).
CREATE TABLE notifications (
    id          TEXT PRIMARY KEY,           -- random, as `mentions`: order by rowid, which grows with time
    user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL,
    page_id     TEXT NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    comment_id  TEXT REFERENCES comments(id) ON DELETE CASCADE,  -- NULL: a mention in the body
    actor_id    TEXT REFERENCES users(id) ON DELETE SET NULL,
    read_at     BIGINT,
    created_at  BIGINT NOT NULL
);
CREATE INDEX notifications_user ON notifications(user_id, read_at);
CREATE INDEX notifications_page ON notifications(page_id);
CREATE INDEX notifications_comment ON notifications(comment_id);
