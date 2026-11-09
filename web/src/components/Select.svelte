<script lang="ts">
  import type { HTMLSelectAttributes } from "svelte/elements";

  import type { Option } from "../lib/roles";

  interface Props extends HTMLSelectAttributes {
    label: string;
    value?: string;
    options: Option[];
    /** What the server refused about this field. */
    problem?: string | null;
  }

  let { label, value = $bindable(""), options, problem = null, ...select }: Props = $props();
</script>

<label class="field">
  <span class="label">{label}</span>
  <select bind:value aria-invalid={problem ? "true" : undefined} {...select}>
    {#each options as option (option.value)}
      <option value={option.value}>{option.label}</option>
    {/each}
  </select>
  {#if problem}<span class="problem" role="alert">{problem}</span>{/if}
</label>
