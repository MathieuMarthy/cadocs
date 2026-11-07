<script lang="ts">
  import type { HTMLInputAttributes } from "svelte/elements";

  interface Props extends HTMLInputAttributes {
    label: string;
    value?: string;
    /** What the server refused about this field. */
    problem?: string | null;
  }

  let { label, value = $bindable(""), problem = null, ...input }: Props = $props();
</script>

<label class="field">
  <span class="label">{label}</span>
  <input bind:value aria-invalid={problem ? "true" : undefined} {...input} />
  {#if problem}<span class="problem" role="alert">{problem}</span>{/if}
</label>
