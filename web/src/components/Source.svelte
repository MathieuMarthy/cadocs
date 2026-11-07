<script lang="ts">
  import { nextTheme, theme } from "../lib/theme.svelte";
</script>

<!-- ADR 0010: every screen offers the source of the running version. -->
<div class="actions ui">
  <button class="button secondary" type="button" onclick={nextTheme}>theme: {theme.current}</button>
  <a class="button secondary" href={__SOURCE_URL__} rel="noreferrer">source</a>
  <span class="meta" style="margin: 0">agpl-3.0 · {__VERSION__}</span>
</div>
