CREATE TABLE jobs (
    id          TEXT PRIMARY KEY,
    kind        TEXT NOT NULL,
    dedup_key   TEXT,                        -- e.g. 'reindex:<page id>'
    payload     TEXT NOT NULL DEFAULT '{}',  -- JSON
    status      TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'running', 'failed')),
    attempts    INTEGER NOT NULL DEFAULT 0,
    run_at      BIGINT NOT NULL,
    locked_at   BIGINT,
    last_error  TEXT,
    created_at  BIGINT NOT NULL
);
CREATE INDEX jobs_due ON jobs(status, run_at);
CREATE UNIQUE INDEX jobs_dedup ON jobs(dedup_key) WHERE dedup_key IS NOT NULL AND status = 'pending';
