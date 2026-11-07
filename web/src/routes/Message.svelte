<script lang="ts">
  import type { Snippet } from "svelte";

  import Solo from "../components/Solo.svelte";

  let { title, children }: { title: string; children?: Snippet } = $props();
</script>

<Solo {title}>
  {#if children}
    {@render children()}
  {:else}
    <a class="button" href="/">go to cadocs</a>
  {/if}
</Solo>
