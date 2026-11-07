<script lang="ts">
  import type { HTMLButtonAttributes } from "svelte/elements";

  import Icon, { type IconName } from "./Icon.svelte";

  /** A compact action: an icon, its verb in `title`, and the full label for assistive tools. */
  let {
    icon,
    verb,
    label = verb,
    danger = false,
    element = $bindable(),
    class: extra = "",
    ...rest
  }: {
    icon: IconName;
    /** The verb, as everywhere else, shown on hover. */
    verb: string;
    /** What a screen reader says, the verb and what it acts on. */
    label?: string;
    danger?: boolean;
    element?: HTMLButtonElement;
  } & HTMLButtonAttributes = $props();
</script>

<button
  bind:this={element}
  class="icon-button {extra}"
  class:danger
  type="button"
  aria-label={label}
  title={verb}
  {...rest}><Icon name={icon} /></button
>
