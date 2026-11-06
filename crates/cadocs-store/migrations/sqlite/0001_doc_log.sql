-- Prototype subset of docs/DATA_MODEL.md: doc_id replaces page_id, with no foreign key.

CREATE TABLE doc_snapshots (
    doc_id     TEXT PRIMARY KEY,
    state      BLOB NOT NULL,                -- Yjs update v1, full document
    upto_seq   BIGINT NOT NULL,              -- last doc_updates.seq included
    updated_at BIGINT NOT NULL
);

CREATE TABLE doc_updates (
    doc_id     TEXT NOT NULL,
    seq        BIGINT NOT NULL,              -- per document, assigned by the room
    data       BLOB NOT NULL,                -- Yjs update v1
    created_at BIGINT NOT NULL,
    PRIMARY KEY (doc_id, seq)
);
