// The pending invitations of the person logged in (step 74): the workspace selector lists them.
// Read once per session read, and again on each `invitation` event.

import { api, type Session } from "./api";
import { collect } from "./nav.svelte";
import { navigate } from "./router.svelte";
import { app, signIn } from "./session.svelte";

export interface PendingInvitation {
  id: string;
  workspace_id: string;
  workspace_name: string;
  role: string;
  invited_by_name: string;
  expires_at: number;
  created_at: number;
}

export const pending = $state<{ items: PendingInvitation[] }>({ items: [] });

/** The session they were read for: a login, or a link accepted in this tab, reads them again. */
let readFor: Session | null = null;
/** Only the last read writes, so a slow one does not bring back an answered invitation. */
let reads = 0;

/** Reads them for the session on screen, once unless `again`. */
export async function loadInvitations(again = false) {
  const session = app.session;
  if (session === readFor && !again) return;
  if (session?.user.id !== readFor?.user.id) pending.items = [];
  readFor = session;
  if (!session) return;
  const read = ++reads;
  try {
    const { items } = await collect<PendingInvitation>("/me/invitations", 100);
    if (read === reads) pending.items = items;
  } catch {
    // The next shell asks again.
    if (read === reads) readFor = null;
  }
}

/** Joins with an invitation and opens its workspace, which the session it answers lists. */
export async function joinInvitation(invitation: PendingInvitation) {
  try {
    const session = await api<Session>("POST", `/me/invitations/${invitation.id}/accept`);
    drop(invitation);
    await signIn(session);
    const joined = session.workspaces.find((item) => item.id === invitation.workspace_id);
    navigate(joined ? `/w/${joined.slug}` : "/");
  } catch (error) {
    // Revoked or expired meanwhile: the list says so.
    void loadInvitations(true);
    throw error;
  }
}

export async function declineInvitation(invitation: PendingInvitation) {
  try {
    await api("DELETE", `/me/invitations/${invitation.id}`);
    drop(invitation);
  } catch (error) {
    void loadInvitations(true);
    throw error;
  }
}

function drop(invitation: PendingInvitation) {
  pending.items = pending.items.filter((item) => item.id !== invitation.id);
}
