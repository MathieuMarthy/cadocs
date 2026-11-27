-- The account that held the invited address when the invitation was made (step 73): only it sees
-- the invitation by id. An address taken afterwards needs the link. Older invitations have none.
ALTER TABLE workspace_invitations ADD COLUMN invitee_id TEXT REFERENCES users(id) ON DELETE SET NULL;
CREATE INDEX workspace_invitations_invitee ON workspace_invitations(invitee_id);
