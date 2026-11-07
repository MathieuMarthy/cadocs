// The one short message the interface shows after an action, with a way to take it back.
// It lives here rather than in a screen, since each screen mounts its own shell.

import { describe } from "./form.svelte";

export interface Notice {
  id: number;
  text: string;
  /** What `undo` does. The notice takes focus, so a keyboard reaches it at once. */
  undo?: () => Promise<unknown>;
  /** Taken once, by the first shell that shows the notice. */
  focus: boolean;
}

/** How long a notice stays, unless the pointer or focus holds it. */
const SHOWN_MS = 8000;

export const notices = $state<{ current: Notice | null }>({ current: null });

let count = 0;
let timer: ReturnType<typeof setTimeout> | null = null;

export function notify(text: string, undo?: () => Promise<unknown>) {
  notices.current = { id: ++count, text, undo, focus: undo !== undefined };
  release();
}

export function dismiss() {
  hold();
  notices.current = null;
}

/** Keeps the notice while it is pointed at or focused. */
export function hold() {
  if (timer) clearTimeout(timer);
  timer = null;
}

export function release() {
  hold();
  const id = notices.current?.id;
  timer = setTimeout(() => {
    if (notices.current?.id === id) notices.current = null;
  }, SHOWN_MS);
}

export async function undo(notice: Notice) {
  try {
    await notice.undo?.();
    if (notices.current?.id === notice.id) dismiss();
  } catch (error) {
    if (notices.current?.id === notice.id) notify(describe(error));
  }
}
