-- Pages linked by another page's body (step 40), as the indexing job finds them: one row per
-- source and target, only for targets of the source's workspace. The backlinks of a page.
CREATE TABLE page_links (
    source_id TEXT NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    target_id TEXT NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    PRIMARY KEY (source_id, target_id)
) WITHOUT ROWID;
CREATE INDEX page_links_target ON page_links(target_id);
