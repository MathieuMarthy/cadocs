CREATE TABLE workspace_invitations (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    email        TEXT NOT NULL,                -- stored lowercased
    role         TEXT NOT NULL CHECK (role IN ('admin', 'member')),
    token_hash   TEXT NOT NULL UNIQUE,
    invited_by   TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at   BIGINT NOT NULL,
    created_at   BIGINT NOT NULL,
    UNIQUE (workspace_id, email)
);
