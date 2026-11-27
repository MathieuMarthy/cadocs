<script lang="ts">
  import { onMount } from "svelte";

  import { t } from "../lib/i18n";

  /** A load faster than this ends before the skeleton shows, so it does not flash. */
  const DELAY_MS = 300;

  /** How many bars each kind draws. */
  const BARS = { page: 6, rows: 4, records: 6 };

  let {
    kind,
    label = t("common.loading"),
    since = performance.now(),
  }: {
    kind: keyof typeof BARS;
    label?: string;
    /** When the load started, so two skeletons in a row show as one. */
    since?: number;
  } = $props();

  let shown = $state(false);
  onMount(() => {
    const timer = setTimeout(() => (shown = true), since + DELAY_MS - performance.now());
    return () => clearTimeout(timer);
  });
</script>

<div class="skeleton" data-kind={kind} role="status" hidden={!shown}>
  <span class="skeleton-label">{label}</span>
  {#each { length: BARS[kind] }, index (index)}
    <span class="skeleton-bar" aria-hidden="true"></span>
  {/each}
</div>
