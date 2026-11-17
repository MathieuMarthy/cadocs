-- Comments of a page (step 37). An anchor is two Yjs relative positions the server stores as given
-- (ADR 0012): all three anchor columns are NULL for a reply or a comment on the whole page.
CREATE TABLE comments (
    id           TEXT PRIMARY KEY,
    page_id      TEXT NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    parent_id    TEXT REFERENCES comments(id) ON DELETE CASCADE, -- one level of replies
    author_id    TEXT REFERENCES users(id) ON DELETE SET NULL,
    body         TEXT NOT NULL,              -- Markdown
    anchor_start BLOB,
    anchor_end   BLOB,
    anchor_text  TEXT,
    resolved_at  BIGINT,
    resolved_by  TEXT REFERENCES users(id) ON DELETE SET NULL,
    created_at   BIGINT NOT NULL,
    updated_at   BIGINT NOT NULL
);
-- Threads of a page in order, ids being UUIDv7; replies by their thread.
CREATE INDEX comments_page ON comments(page_id, parent_id, id);
CREATE INDEX comments_parent ON comments(parent_id, id);
