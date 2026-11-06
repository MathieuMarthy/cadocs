CREATE TABLE pages (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    space_id     TEXT NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
    parent_id    TEXT REFERENCES pages(id) ON DELETE CASCADE,
    position     TEXT NOT NULL,
    slug         TEXT NOT NULL,              -- short random id for URLs
    title        TEXT NOT NULL DEFAULT '',   -- copy of the Yjs title
    created_by   TEXT REFERENCES users(id) ON DELETE SET NULL,
    updated_by   TEXT REFERENCES users(id) ON DELETE SET NULL,
    deleted_at   BIGINT,
    deleted_by   TEXT REFERENCES users(id) ON DELETE SET NULL,
    created_at   BIGINT NOT NULL,
    updated_at   BIGINT NOT NULL,
    UNIQUE (workspace_id, slug)
);
CREATE INDEX pages_tree ON pages(space_id, parent_id, position);
CREATE INDEX pages_parent ON pages(parent_id);
CREATE INDEX pages_recent ON pages(workspace_id, updated_at);
-- The tree cursor and `between` both rely on it.
CREATE UNIQUE INDEX pages_sibling_keys ON pages(coalesce(parent_id, space_id), position);
CREATE INDEX pages_trash ON pages(space_id, deleted_at) WHERE deleted_at IS NOT NULL;
