import type * as Y from "yjs";

type Delta = Y.YTextEvent["delta"];

/** Keeps a text input and a `Y.Text` equal, in both directions. */
export function bindTitle(input: HTMLInputElement, text: Y.Text, onChange: (title: string) => void) {
  input.addEventListener("input", () => {
    const { index, removed, inserted } = diff(text.toString(), input.value);
    text.doc?.transact(() => {
      if (removed > 0) text.delete(index, removed);
      if (inserted) text.insert(index, inserted);
    });
    onChange(input.value);
  });

  text.observe((event) => {
    const title = text.toString();
    onChange(title);
    if (input.value === title) return;
    const start = shift(input.selectionStart ?? 0, event.delta);
    const end = shift(input.selectionEnd ?? 0, event.delta);
    input.value = title;
    input.setSelectionRange(start, end);
  });
}

/** The single replacement that turns `before` into `after`. */
function diff(before: string, after: string) {
  const longest = Math.min(before.length, after.length);
  let start = 0;
  while (start < longest && before[start] === after[start]) start++;
  if (start > 0 && isHighSurrogate(before.charCodeAt(start - 1))) start--;

  let end = 0;
  while (
    end < longest - start &&
    before[before.length - 1 - end] === after[after.length - 1 - end]
  ) {
    end++;
  }
  if (end > 0 && isLowSurrogate(before.charCodeAt(before.length - end))) end--;

  return {
    index: start,
    removed: before.length - start - end,
    inserted: after.slice(start, after.length - end),
  };
}

const isHighSurrogate = (unit: number) => unit >= 0xd800 && unit <= 0xdbff;
const isLowSurrogate = (unit: number) => unit >= 0xdc00 && unit <= 0xdfff;

/** Where a caret at `position` ends up after a remote change. */
function shift(position: number, delta: Delta) {
  let index = 0;
  let result = position;
  for (const op of delta) {
    if (op.retain) {
      index += op.retain;
    } else if (typeof op.insert === "string") {
      if (index <= result) result += op.insert.length;
      index += op.insert.length;
    } else if (op.delete) {
      if (index < result) result -= Math.min(op.delete, result - index);
    }
  }
  return result;
}
