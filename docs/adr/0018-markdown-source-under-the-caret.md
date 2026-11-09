# ADR 0018: Markdown source under the caret, written into the document

Status: accepted

## Context

Step 14 turned Markdown shortcuts into blocks: `## ` makes a heading, `[ ] ` a task, `---` a divider. Once made, the source was gone. Changing a heading's level, unchecking a task by its brackets or turning a divider back into text meant going through the slash menu, and a divider undone with Backspace stayed as `---` text for good. Users coming from Obsidian expect the source to come back where the caret is, and to be edited as text.

The shared document holds blocks, not Markdown text, so the source has to be drawn by the editor or written into the document. Two models were weighed:

- **A marker drawn by the editor.** The source is a widget. Keys typed on it change the block: `#` adds a level, `x` checks the task. Nothing moves for the others and nested lists stay intact, but the source is not text: only the planned keys do anything.
- **The source written as text.** Arrowing onto the marker writes the source into the block as text. It is edited freely, and read again when the caret leaves the block.

## Decision

The source is written as text (the second model), in `editor/source.ts`:

- A heading or quote paragraph under the caret of a focused writer shows its marker (`##`, `>`) beside it, drawn by that tab alone, in place of the heading number.
- ArrowLeft at the start of a block, a click on the marker, or Backspace at the start (which then deletes the source's last character) writes the source into the block: `## ` in a heading, `[ ] ` or `[x] ` in a task, `- ` or `N. ` in a list item, `> ` in a quote paragraph, the fence line in code. A divider the caret lands on becomes the text `---`.
- When the caret leaves the block, its text is read again and the block becomes what it says: a heading of another level, a paragraph, a checked task, a bullet, a list item out of its list, a code block in another language or plain lines. A block this tab edited that starts with its source, such as a paragraph whose shortcut was undone, is read the same way. A block it did not edit is never read: another tab may be writing its source out, and code may hold Markdown on purpose.
- While a block is written out, the block shortcuts stay silent in it, so that `- ` typed there is text and not a nested list.

Every heading level carries a number, counted from the highest level used on the page (`01`, `01.1`, `01.1.1`), drawn from a `data-number` attribute the editor sets. A code block shows its language, or `auto` when highlighting guesses it.

## Consequences

- Other users see the source as text while it is edited: `## Title` in a heading, `- item` beside a bullet. It is the price of free editing.
- A tab that closes while a block is written out reads it on `pagehide` and on leaving the page. If that write does not reach the server, the source stays as text until someone edits the block and leaves it.
- A key pressed right after Home, End or a click can reach the editor before it reads the moved selection. The source plugin reads the page's own selection first, for its keys and for those of the keymaps after it.
- Another tab's change can rebuild the document. The block being written out is then found again from the caret, if its text is the one this tab last saw or still reads as source. When this tab has removed the marker and another tab types in that same block at that moment, neither holds: the block keeps its type, a heading stays a heading. It is a known limit.
- Undo goes through the steps one by one: writing out, editing, reading again. An undo arrives through Yjs like another tab's change, and is still treated as this tab's.
- Undoing a list shortcut leaves `- item` as text only until the caret leaves: the line is then read as a list, as Obsidian would show it.
