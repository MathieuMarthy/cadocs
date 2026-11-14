<script lang="ts">
  import { appear } from "../lib/motion";
  import type { IconName } from "./icons";
  import IconButton from "./IconButton.svelte";

  /** A button that asks once more, for what cannot be undone. */
  let {
    label,
    confirm,
    icon,
    danger = false,
    disabled = false,
    onconfirm,
  }: {
    label: string;
    confirm: string;
    /** Shows the first button as an icon, with `label` as its verb. */
    icon?: IconName;
    /** A final action: the confirmation is in `danger`. */
    danger?: boolean;
    disabled?: boolean;
    onconfirm: () => void;
  } = $props();

  let asking = $state(false);
</script>

<!-- Each branch fades in and the one it replaces goes at once, so both never show side by side. -->
{#if asking}
  <button
    class="button"
    class:danger
    type="button"
    {disabled}
    in:appear
    onclick={() => {
      asking = false;
      onconfirm();
    }}
  >
    {confirm}
  </button>
  <button class="button secondary" type="button" in:appear onclick={() => (asking = false)}>cancel</button>
{:else if icon}
  <IconButton {icon} verb={label} {danger} {disabled} appear onclick={() => (asking = true)} />
{:else}
  <button
    class="button"
    class:secondary={!danger}
    class:danger
    type="button"
    {disabled}
    in:appear
    onclick={() => (asking = true)}
  >
    {label}
  </button>
{/if}
