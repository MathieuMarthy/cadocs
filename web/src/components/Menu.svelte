<script lang="ts" module>
  import type { IconName } from "./icons";

  export interface MenuItem {
    icon: IconName;
    /** The verb, in text. */
    label: string;
    /** A link instead of an action. */
    href?: string;
    select?: () => void;
    /** Goes last, below a rule. */
    destructive?: boolean;
    /** A final action, in `danger`. */
    danger?: boolean;
  }
</script>

<script lang="ts">
  import { tick } from "svelte";

  import Icon from "./Icon.svelte";
  import IconButton from "./IconButton.svelte";

  /**
   * The `ellipsis` button and the action menu it opens below it, aligned to its right. With
   * `text`, the button shows that text instead, as the user menu shows the user's name.
   */
  let {
    label,
    items,
    text,
    class: extra = "",
  }: { label: string; items: MenuItem[]; text?: string; class?: string } = $props();

  const id = $props.id();
  let open = $state(false);
  let button = $state<HTMLButtonElement>();
  let menu = $state<HTMLElement>();

  const kept = $derived(items.filter((item) => !item.destructive));
  const destructive = $derived(items.filter((item) => item.destructive));

  const entries = () => [...(menu?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])];

  /**
   * A keyboard opening marks the first item. A pointer opening marks none, so no row looks
   * chosen before the pointer reaches it; the menu holds focus and the arrows still work.
   */
  async function show(byKeyboard: boolean) {
    open = true;
    await tick();
    if (!menu || !button) return;
    menu.showPopover();
    const box = button.getBoundingClientRect();
    const left = Math.max(0, box.right - menu.offsetWidth);
    const below = box.bottom + menu.offsetHeight <= window.innerHeight;
    menu.style.left = `${left}px`;
    menu.style.top = `${below ? box.bottom : Math.max(0, box.top - menu.offsetHeight)}px`;
    if (byKeyboard) entries()[0]?.focus();
    else menu.focus();
  }

  /** A click from Enter or Space has no pointer position: `detail` is 0. */
  const toggle = (event: MouseEvent) => (open ? hide() : void show(event.detail === 0));

  /** Closes the menu, giving focus back to its button unless it went elsewhere. */
  function hide(refocus = true) {
    if (!open) return;
    open = false;
    if (refocus) button?.focus();
  }

  function choose(item: MenuItem) {
    hide();
    item.select?.();
  }

  function key(event: KeyboardEvent) {
    const list = entries();
    const at = list.indexOf(document.activeElement as HTMLElement);
    const move = (index: number) => list[(index + list.length) % list.length]?.focus();
    switch (event.key) {
      case "ArrowDown":
        move(at + 1);
        break;
      case "ArrowUp":
        move(at < 0 ? -1 : at - 1);
        break;
      case "Home":
        move(0);
        break;
      case "End":
        move(-1);
        break;
      case "Escape":
      case "Tab":
        hide();
        break;
      default:
        return;
    }
    event.preventDefault();
  }

  /** A press outside, or the page scrolling under it, closes the menu. */
  function outside(event: Event) {
    const target = event.target as Node | null;
    if (target && (menu?.contains(target) || button?.contains(target))) return;
    hide(false);
  }

  $effect(() => {
    if (!open) return;
    window.addEventListener("pointerdown", outside, true);
    window.addEventListener("scroll", outside, true);
    window.addEventListener("resize", outside);
    return () => {
      window.removeEventListener("pointerdown", outside, true);
      window.removeEventListener("scroll", outside, true);
      window.removeEventListener("resize", outside);
    };
  });
</script>

{#snippet entry(item: MenuItem)}
  {#if item.href}
    <a
      class="menu-item"
      class:danger={item.danger}
      role="menuitem"
      tabindex="-1"
      href={item.href}
      onclick={() => hide(false)}
      onpointermove={(event) => event.currentTarget.focus()}
      ><Icon name={item.icon} />{item.label}</a
    >
  {:else}
    <button
      class="menu-item"
      class:danger={item.danger}
      type="button"
      role="menuitem"
      tabindex="-1"
      onclick={() => choose(item)}
      onpointermove={(event) => event.currentTarget.focus()}
      ><Icon name={item.icon} />{item.label}</button
    >
  {/if}
{/snippet}

{#if text}
  <button
    bind:this={button}
    class="menu-trigger {extra}"
    type="button"
    aria-haspopup="menu"
    aria-expanded={open}
    aria-controls={open ? id : undefined}
    onclick={toggle}>{text}</button
  >
{:else}
  <IconButton
    bind:element={button}
    class={extra}
    icon="ellipsis"
    verb="more actions"
    {label}
    aria-haspopup="menu"
    aria-expanded={open}
    aria-controls={open ? id : undefined}
    onclick={toggle}
  />
{/if}
{#if open}
  <div
    bind:this={menu}
    {id}
    class="menu"
    role="menu"
    tabindex="-1"
    aria-label={label}
    popover="manual"
    onkeydown={key}
    onfocusout={(event) => {
      const next = event.relatedTarget as Node | null;
      if (next && !menu?.contains(next) && next !== button) hide(false);
    }}
  >
    {#each kept as item (item.label)}{@render entry(item)}{/each}
    {#if kept.length && destructive.length}<div class="menu-rule" role="separator"></div>{/if}
    {#each destructive as item (item.label)}{@render entry(item)}{/each}
  </div>
{/if}
