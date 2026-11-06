-- A document now belongs to a page. The prototype's documents have no page, and go.

CREATE TABLE doc_snapshots_new (
    page_id    TEXT PRIMARY KEY REFERENCES pages(id) ON DELETE CASCADE,
    state      BLOB NOT NULL,                -- Yjs update v1, full document
    upto_seq   BIGINT NOT NULL,              -- last doc_updates.seq included
    updated_at BIGINT NOT NULL
);
INSERT INTO doc_snapshots_new
    SELECT doc_id, state, upto_seq, updated_at FROM doc_snapshots
    WHERE doc_id IN (SELECT id FROM pages);
DROP TABLE doc_snapshots;
ALTER TABLE doc_snapshots_new RENAME TO doc_snapshots;

CREATE TABLE doc_updates_new (
    page_id    TEXT NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    seq        BIGINT NOT NULL,              -- per page, assigned by the room
    data       BLOB NOT NULL,                -- Yjs update v1
    created_at BIGINT NOT NULL,
    PRIMARY KEY (page_id, seq)
);
INSERT INTO doc_updates_new
    SELECT doc_id, seq, data, created_at FROM doc_updates
    WHERE doc_id IN (SELECT id FROM pages);
DROP TABLE doc_updates;
ALTER TABLE doc_updates_new RENAME TO doc_updates;
