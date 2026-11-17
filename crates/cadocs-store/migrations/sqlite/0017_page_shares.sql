-- Public links to a page (step 45). The token is the capability, so it stays readable for the
-- editors who list it again.
CREATE TABLE page_shares (
    id                 TEXT PRIMARY KEY,
    page_id            TEXT NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    token              TEXT NOT NULL UNIQUE,  -- 128 random bits, in the public URL
    include_children   SMALLINT NOT NULL DEFAULT 0 CHECK (include_children IN (0, 1)),
    indexable          SMALLINT NOT NULL DEFAULT 0 CHECK (indexable IN (0, 1)), -- allow search engines
    created_by         TEXT REFERENCES users(id) ON DELETE SET NULL,
    expires_at         BIGINT,
    created_at         BIGINT NOT NULL
);
CREATE INDEX page_shares_page ON page_shares(page_id);

-- The attachments a page's content shows now, as its last index found them. `attachment_refs`
-- only grows, since a revision may still show a file; a share opens only the files here.
CREATE TABLE page_attachments (
    page_id       TEXT NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    attachment_id TEXT NOT NULL REFERENCES attachments(id) ON DELETE CASCADE,
    PRIMARY KEY (page_id, attachment_id)
);
CREATE INDEX page_attachments_attachment ON page_attachments(attachment_id);

-- Pages indexed before this table: each gets an index, as an edit would queue it.
INSERT INTO jobs (id, kind, dedup_key, payload, run_at, created_at)
SELECT lower(hex(randomblob(16))), 'reindex_page', 'reindex:' || id, json_object('page', id),
       strftime('%s', 'now') * 1000, strftime('%s', 'now') * 1000
FROM pages WHERE true
ON CONFLICT DO NOTHING;
