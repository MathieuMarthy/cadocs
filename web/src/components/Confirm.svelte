<script lang="ts">
  /** A button that asks once more, for what cannot be undone. */
  let {
    label,
    confirm,
    disabled = false,
    onconfirm,
  }: { label: string; confirm: string; disabled?: boolean; onconfirm: () => void } = $props();

  let asking = $state(false);
</script>

{#if asking}
  <button
    class="button"
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
{:else}
  <button class="button secondary" type="button" {disabled} onclick={() => (asking = true)}>
    {label}
  </button>
{/if}
