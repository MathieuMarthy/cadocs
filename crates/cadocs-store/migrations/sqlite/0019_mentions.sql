-- Users named by a page (step 39): in its body, as the indexing job finds them, or in a comment,
-- as its body gives them. One row per user and place, only for members of the page's workspace.
CREATE TABLE mentions (
    id          TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,  -- who is mentioned
    page_id     TEXT NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    comment_id  TEXT REFERENCES comments(id) ON DELETE CASCADE,        -- NULL: in the page body
    author_id   TEXT REFERENCES users(id) ON DELETE SET NULL,
    read_at     BIGINT,
    created_at  BIGINT NOT NULL
);
CREATE INDEX mentions_user ON mentions(user_id, read_at);
CREATE INDEX mentions_page ON mentions(page_id);
CREATE INDEX mentions_comment ON mentions(comment_id);
