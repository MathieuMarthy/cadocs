<script lang="ts">
  import type { IconName } from "./Icon.svelte";
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

{#if asking}
  <button
    class="button"
    class:danger
    type="button"
    {disabled}
    onclick={() => {
      asking = false;
      onconfirm();
    }}
  >
    {confirm}
  </button>
  <button class="button secondary" type="button" onclick={() => (asking = false)}>cancel</button>
{:else if icon}
  <IconButton {icon} verb={label} {danger} {disabled} onclick={() => (asking = true)} />
{:else}
  <button class="button secondary" type="button" {disabled} onclick={() => (asking = true)}>
    {label}
  </button>
{/if}
