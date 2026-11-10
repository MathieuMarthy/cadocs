-- Revisions of a page (docs/DATA_MODEL.md#documents): full states, kept by a job after edits.
CREATE TABLE page_revisions (
    id         TEXT PRIMARY KEY,
    page_id    TEXT NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    number     BIGINT NOT NULL,
    title      TEXT NOT NULL,
    state      BLOB NOT NULL,                -- zstd-compressed Yjs state at that moment
    authors    TEXT NOT NULL,                -- JSON array of user ids since the last revision
    created_at BIGINT NOT NULL,
    UNIQUE (page_id, number)
);

-- Who wrote to a page since its last revision. A flush adds its author, and a revision takes the
-- rows its read covers, so an edit made during that read still counts for the next one.
CREATE TABLE revision_authors (
    page_id    TEXT NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    last_at    BIGINT NOT NULL,              -- the flush that last added this user
    PRIMARY KEY (page_id, user_id)
) WITHOUT ROWID;
