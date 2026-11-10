CREATE TABLE attachments (
    id           TEXT PRIMARY KEY,           -- also the storage key
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    file_name    TEXT NOT NULL,              -- display only, never a path
    content_type TEXT NOT NULL,
    size         BIGINT NOT NULL,
    sha256       TEXT NOT NULL,
    uploaded_by  TEXT REFERENCES users(id) ON DELETE SET NULL,
    created_at   BIGINT NOT NULL
);
CREATE INDEX attachments_workspace ON attachments(workspace_id);
CREATE INDEX attachments_uploaded_by ON attachments(uploaded_by);

CREATE TABLE attachment_refs (
    attachment_id TEXT NOT NULL REFERENCES attachments(id) ON DELETE CASCADE,
    page_id       TEXT NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    PRIMARY KEY (attachment_id, page_id)
);
CREATE INDEX attachment_refs_page ON attachment_refs(page_id);
