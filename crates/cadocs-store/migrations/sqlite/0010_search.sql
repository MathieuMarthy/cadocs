-- Full-text search (ADR 0007). `page_fts` holds stemmed text, `page_text` the real words.
CREATE TABLE page_text (
    id         INTEGER PRIMARY KEY,          -- also the page_fts rowid
    page_id    TEXT NOT NULL UNIQUE REFERENCES pages(id) ON DELETE CASCADE,
    plain_text TEXT NOT NULL,                -- original words, for excerpts
    indexed_at BIGINT NOT NULL
);

CREATE VIRTUAL TABLE page_fts USING fts5(
    title,                                   -- stemmed
    body,                                    -- stemmed
    tokenize = "unicode61 remove_diacritics 2",
    prefix = '2 3'
);

CREATE TRIGGER page_text_delete AFTER DELETE ON page_text BEGIN
    DELETE FROM page_fts WHERE rowid = old.id;
END;

-- Pages written before search: each gets its first index, as an edit would queue it.
INSERT INTO jobs (id, kind, dedup_key, payload, run_at, created_at)
SELECT lower(hex(randomblob(16))), 'reindex_page', 'reindex:' || id, json_object('page', id),
       strftime('%s', 'now') * 1000, strftime('%s', 'now') * 1000
FROM pages WHERE true
ON CONFLICT DO NOTHING;
