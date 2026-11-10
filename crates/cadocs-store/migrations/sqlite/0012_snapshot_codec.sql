-- How `doc_snapshots.state` is encoded: 0 for a raw Yjs update v1, 1 for that update compressed
-- with zstd. Rows written before this stay raw until their page compacts again.
ALTER TABLE doc_snapshots ADD COLUMN codec SMALLINT NOT NULL DEFAULT 0;
