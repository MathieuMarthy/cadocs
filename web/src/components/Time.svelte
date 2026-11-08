<script lang="ts">
  import { onMount } from "svelte";

  import { exact, relative } from "../lib/time";

  /** A date as `5 minutes ago`, with the exact one on hover. */
  let { value }: { value: string | number } = $props();

  const date = $derived(new Date(value));
  let now = $state(Date.now());

  onMount(() => {
    const timer = setInterval(() => (now = Date.now()), 30_000);
    return () => clearInterval(timer);
  });
</script>

<time datetime={date.toISOString()} title={exact(date)}>{relative(date, now)}</time>
