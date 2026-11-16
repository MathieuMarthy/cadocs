<script lang="ts">
  import type { Snippet } from "svelte";

  import Solo from "../components/Solo.svelte";
  import { t } from "../lib/i18n";

  let { title, children }: { title: string; children?: Snippet } = $props();
</script>

<Solo {title}>
  {#if children}
    {@render children()}
  {:else}
    <a class="button" href="/">{t("app.home")}</a>
  {/if}
</Solo>
