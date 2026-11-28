<script lang="ts" module>
  import { releaseNotes } from "../lib/i18n";

  const KEY = "cadocs-version";
  const NONE = { features: [], fixes: [] };

  /** Read once per page load: a screen drawn again, as a change of language does, keeps it until closed. */
  let unseen: ReturnType<typeof releaseNotes> | null = null;

  function read() {
    try {
      const seen = localStorage.getItem(KEY);
      localStorage.setItem(KEY, __VERSION__);
      return releaseNotes(seen, __VERSION__);
    } catch {
      return NONE; // Storage can be blocked: no release to compare with.
    }
  }
</script>

<script lang="ts">
  import { onMount } from "svelte";
  import { t } from "../lib/i18n";
  import IconButton from "./IconButton.svelte";

  /** What changed in this release (step 81), shown once per browser after an upgrade. */
  unseen ??= read();
  const notes = unseen;
  const kinds = [
    { heading: t("news.features"), keys: notes.features },
    { heading: t("news.fixes"), keys: notes.fixes },
  ].filter((kind) => kind.keys.length > 0);
  const headingId = $props.id();
  let dialog = $state<HTMLDialogElement>();

  onMount(() => dialog?.showModal());
</script>

{#if kinds.length}
  <dialog
    bind:this={dialog}
    class="dialog news-dialog"
    aria-labelledby={headingId}
    onclose={() => (unseen = NONE)}
  >
    <div class="dialog-head">
      <h2 id={headingId}>{t("news.title", { version: __VERSION__ })}</h2>
      <IconButton icon="x" verb={t("news.close")} onclick={() => dialog?.close()} />
    </div>
    {#each kinds as kind (kind.heading)}
      <h3 class="label">{kind.heading}</h3>
      <ul>
        {#each kind.keys as key (key)}<li>{t(key)}</li>{/each}
      </ul>
    {/each}
  </dialog>
{/if}
