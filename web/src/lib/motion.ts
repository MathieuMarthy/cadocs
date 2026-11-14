import { linear } from "svelte/easing";
import { fade, slide, type TransitionConfig } from "svelte/transition";

/**
 * The `motion-duration` token in milliseconds, read at each transition so a change of the
 * reduced motion setting applies at once. It is 0 under reduced motion, and Svelte runs no
 * animation at all for a zero duration.
 */
export function duration(): number {
  const value = getComputedStyle(document.documentElement)
    .getPropertyValue("--motion-duration")
    .trim();
  const amount = Number.parseFloat(value);
  if (!Number.isFinite(amount) || amount <= 0) return 0;
  return value.endsWith("ms") ? amount : amount * 1000;
}

/** A tree level opening or closing: its height, linear. */
export const unfold = (node: Element): TransitionConfig =>
  slide(node, { duration: duration(), easing: linear });

/** A surface appearing or leaving in place: menus, the notice, the confirm buttons. */
export const appear = (node: Element, on = true): TransitionConfig =>
  fade(node, { duration: on ? duration() : 0, easing: linear });
