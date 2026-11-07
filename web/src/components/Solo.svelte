<script lang="ts">
  import type { Snippet } from "svelte";

  import Source from "./Source.svelte";

  let { title, children }: { title: string; children: Snippet } = $props();
</script>

<svelte:head><title>{title} · cadocs</title></svelte:head>

<div class="solo">
  <a class="solo-brand" href="/"><span class="logo" aria-hidden="true"></span>cadocs</a>
  <main>
    <h1>{title}</h1>
    {@render children()}
  </main>
  <div class="solo-end"><Source /></div>
</div>
