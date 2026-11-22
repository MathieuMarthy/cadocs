export interface Part {
  kind: "same" | "add" | "del";
  block: string;
}

/** Blocks past which the table is not built: everything changed is shown as removed then added. */
const LIMIT = 4_000_000;

/** Longest common subsequence of whole blocks, after trimming what both ends share. */
export function diffBlocks(before: string[], after: string[]): Part[] {
  let head = 0;
  while (head < before.length && head < after.length && before[head] === after[head]) head++;
  let tail = 0;
  while (
    tail < before.length - head &&
    tail < after.length - head &&
    before[before.length - 1 - tail] === after[after.length - 1 - tail]
  )
    tail++;
  const start: Part[] = before.slice(0, head).map((block) => ({ kind: "same", block }));
  const end: Part[] = before.slice(before.length - tail).map((block) => ({ kind: "same", block }));
  return [...start, ...middle(before.slice(head, before.length - tail), after.slice(head, after.length - tail)), ...end];
}

function middle(before: string[], after: string[]): Part[] {
  const n = before.length;
  const m = after.length;
  if ((n + 1) * (m + 1) > LIMIT)
    return [...before.map((block): Part => ({ kind: "del", block })), ...after.map((block): Part => ({ kind: "add", block }))];
  const width = m + 1;
  const table = new Uint32Array((n + 1) * width);
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      table[i * width + j] =
        before[i] === after[j]
          ? table[(i + 1) * width + j + 1]! + 1
          : Math.max(table[(i + 1) * width + j]!, table[i * width + j + 1]!);
    }
  }
  const parts: Part[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (before[i] === after[j]) parts.push({ kind: "same", block: before[i++]! }), j++;
    else if (table[(i + 1) * width + j]! >= table[i * width + j + 1]!) parts.push({ kind: "del", block: before[i++]! });
    else parts.push({ kind: "add", block: after[j++]! });
  }
  while (i < n) parts.push({ kind: "del", block: before[i++]! });
  while (j < m) parts.push({ kind: "add", block: after[j++]! });
  return parts;
}

/** Top-level blocks of the server's escaped HTML, as markup. */
export function blocksOf(html: string): string[] {
  const holder = document.createElement("template");
  holder.innerHTML = html;
  return Array.from(holder.content.children, (child) => child.outerHTML);
}

/** The parts as one markup, changed blocks wrapped in a div the stylesheet marks. */
export function markup(parts: Part[]): string {
  // i18n-ignore: markup, not text
  const wrap = (kind: string, block: string) => `<div class="diff-${kind}" role="${kind === "add" ? "insertion" : "deletion"}">${block}</div>`;
  return parts.map(({ kind, block }) => (kind === "same" ? block : wrap(kind, block))).join("");
}
