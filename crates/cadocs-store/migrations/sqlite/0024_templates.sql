-- Page templates (step 51): the title and the full Yjs state of a page as it was when saved, kept
-- like a revision, for the editors of a space to start new pages from.
CREATE TABLE templates (
    id         TEXT PRIMARY KEY,
    space_id   TEXT NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
    title      TEXT NOT NULL,
    state      BLOB NOT NULL,                -- zstd-compressed Yjs state
    created_by TEXT REFERENCES users(id) ON DELETE SET NULL,
    created_at BIGINT NOT NULL
);
CREATE INDEX templates_space ON templates(space_id, id);
