-- Accounts subset of docs/DATA_MODEL.md. Later steps add the tables they need.

CREATE TABLE users (
    id             TEXT PRIMARY KEY,
    email          TEXT NOT NULL,            -- stored lowercased
    name           TEXT NOT NULL,
    password_hash  TEXT,                     -- NULL for OIDC-only accounts
    is_instance_admin SMALLINT NOT NULL DEFAULT 0 CHECK (is_instance_admin IN (0, 1)),
    disabled_at    BIGINT,
    created_at     BIGINT NOT NULL,
    updated_at     BIGINT NOT NULL,
    UNIQUE (email)
);

CREATE TABLE sessions (
    token_hash TEXT PRIMARY KEY,             -- SHA-256 of the cookie value
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at BIGINT NOT NULL,
    last_seen_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL
);
CREATE INDEX sessions_user ON sessions(user_id);
CREATE INDEX sessions_expiry ON sessions(expires_at);

CREATE TABLE workspaces (
    id         TEXT PRIMARY KEY,
    slug       TEXT NOT NULL UNIQUE,
    name       TEXT NOT NULL,
    language   TEXT NOT NULL DEFAULT 'en',   -- search stemming (ADR 0007)
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE TABLE workspace_members (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    user_id      TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role         TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member')),
    created_at   BIGINT NOT NULL,
    PRIMARY KEY (workspace_id, user_id)
);
CREATE INDEX workspace_members_user ON workspace_members(user_id);
