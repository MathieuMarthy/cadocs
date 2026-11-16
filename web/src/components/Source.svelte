<script lang="ts">
  import { t } from "../lib/i18n";
  import { nextTheme, theme } from "../lib/theme.svelte";

  /** The shell offers the theme in its user menu, so it leaves it out here. */
  let { withTheme = true }: { withTheme?: boolean } = $props();
</script>

<!-- ADR 0010: every screen offers the source of the running version. -->
<div class="actions ui source-line">
  {#if withTheme}
    <button class="button secondary" type="button" onclick={nextTheme}
      >{t("theme.button", { theme: t(`theme.${theme.current}`) })}</button
    >
  {/if}
  <a class="button secondary" href={__SOURCE_URL__} rel="noreferrer">{t("source.link")}</a>
  <span class="meta">{t("source.license", { version: __VERSION__ })}</span>
</div>
