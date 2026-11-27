-- Prefix search past the stem (step 60): `page_fts` also holds the folded words that stemming
-- changes, once each, so `deploym` finds `deployment`, stemmed `deploy`. FTS5 cannot add a
-- column: the table is made again with the stems it had, and every page is reindexed to fill the
-- words. Renaming would point the trigger at the old table, so it is made again too.
DROP TRIGGER page_text_delete;
ALTER TABLE page_fts RENAME TO page_fts_stems;

CREATE VIRTUAL TABLE page_fts USING fts5(
    title,                                   -- stemmed
    body,                                    -- stemmed
    title_words,                             -- folded words that stemming changes, for prefixes
    body_words,
    tokenize = "unicode61 remove_diacritics 2",
    prefix = '2 3'
);
INSERT INTO page_fts (rowid, title, body, title_words, body_words)
SELECT rowid, title, body, '', '' FROM page_fts_stems;
DROP TABLE page_fts_stems;

CREATE TRIGGER page_text_delete AFTER DELETE ON page_text BEGIN
    DELETE FROM page_fts WHERE rowid = old.id;
END;

INSERT INTO jobs (id, kind, dedup_key, payload, run_at, created_at)
SELECT lower(hex(randomblob(16))), 'reindex_page', 'reindex:' || id, json_object('page', id),
       strftime('%s', 'now') * 1000, strftime('%s', 'now') * 1000
FROM pages WHERE true
ON CONFLICT DO NOTHING;
