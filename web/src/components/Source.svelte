<script lang="ts">
  import { nextTheme, theme } from "../lib/theme.svelte";

  /** The shell offers the theme in its user menu, so it leaves it out here. */
  let { withTheme = true }: { withTheme?: boolean } = $props();
</script>

<!-- ADR 0010: every screen offers the source of the running version. -->
<div class="actions ui source-line">
  {#if withTheme}
    <button class="button secondary" type="button" onclick={nextTheme}>theme: {theme.current}</button>
  {/if}
  <a class="button secondary" href={__SOURCE_URL__} rel="noreferrer">source</a>
  <span class="meta">agpl-3.0 · {__VERSION__}</span>
</div>
