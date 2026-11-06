-- Nothing fills the two group tables before V1, but resolution already reads them.
CREATE TABLE groups (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    created_at   BIGINT NOT NULL,
    UNIQUE (workspace_id, name)
);

CREATE TABLE group_members (
    group_id TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    user_id  TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    PRIMARY KEY (group_id, user_id)
);
CREATE INDEX group_members_user ON group_members(user_id);

CREATE TABLE spaces (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    slug         TEXT NOT NULL,
    name         TEXT NOT NULL,
    description  TEXT NOT NULL DEFAULT '',
    default_role TEXT CHECK (default_role IN ('editor', 'commenter', 'viewer')), -- NULL: invited members only
    deleted_at   BIGINT,
    created_at   BIGINT NOT NULL,
    updated_at   BIGINT NOT NULL,
    UNIQUE (workspace_id, slug)
);

-- A grant targets a user or a group, never both.
CREATE TABLE space_grants (
    id       TEXT PRIMARY KEY,
    space_id TEXT NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
    user_id  TEXT REFERENCES users(id) ON DELETE CASCADE,
    group_id TEXT REFERENCES groups(id) ON DELETE CASCADE,
    role     TEXT NOT NULL CHECK (role IN ('admin', 'editor', 'commenter', 'viewer')),
    created_at BIGINT NOT NULL,
    CHECK ((user_id IS NULL) <> (group_id IS NULL)),
    UNIQUE (space_id, user_id),
    UNIQUE (space_id, group_id)
);
CREATE INDEX space_grants_user ON space_grants(user_id);
CREATE INDEX space_grants_group ON space_grants(group_id);
