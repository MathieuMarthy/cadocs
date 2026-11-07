import type { Awareness } from "y-protocols/awareness";

export interface User {
  name: string;
  /** Which of the three presence colors this user shows in. */
  slot: number;
}

const SLOTS = [1, 2, 3];

export function newUser(): User {
  return { name: `guest-${Math.floor(Math.random() * 900) + 100}`, slot: 1 };
}

/** Awareness states come from other clients, so nothing in them is trusted. */
function readUser(state: Record<string, unknown>): User | null {
  const user = state.user as Partial<User> | undefined;
  if (typeof user?.name !== "string") return null;
  return { name: user.name, slot: slotOf(user) };
}

function slotOf(user: { slot?: unknown }) {
  return SLOTS.includes(user.slot as number) ? (user.slot as number) : 1;
}

/** The color used least by the others, so three people get three colors. */
export function freeSlot(awareness: Awareness) {
  const used = new Map(SLOTS.map((slot) => [slot, 0]));
  for (const [clientId, state] of awareness.getStates()) {
    const user = readUser(state);
    if (user && clientId !== awareness.clientID) {
      used.set(user.slot, (used.get(user.slot) ?? 0) + 1);
    }
  }
  return SLOTS.reduce((best, slot) => (used.get(slot)! < used.get(best)! ? slot : best));
}

export function renderAvatars(container: HTMLElement, awareness: Awareness) {
  const avatars = [...awareness.getStates().values()].flatMap((state) => {
    const user = readUser(state);
    if (!user) return [];
    const avatar = document.createElement("span");
    avatar.className = `avatar slot-${user.slot}`;
    avatar.title = user.name;
    avatar.textContent = user.name.slice(0, 1).toUpperCase();
    return [avatar];
  });
  container.replaceChildren(...avatars);
}

export function renderCaret(user: Record<string, unknown>) {
  const caret = document.createElement("span");
  caret.className = `remote-caret slot-${slotOf(user)}`;
  const label = document.createElement("span");
  label.className = "remote-caret-label";
  label.textContent = String(user.name ?? "").toLowerCase();
  caret.append(label);
  return caret;
}

export function renderSelection(user: Record<string, unknown>) {
  return { class: `remote-selection slot-${slotOf(user)}` };
}
