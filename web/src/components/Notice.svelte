<script lang="ts">
  import { appear } from "../lib/motion";
  import { hold, notices, release, undo } from "../lib/notice.svelte";

  let button = $state<HTMLButtonElement>();

  $effect(() => {
    const notice = notices.current;
    if (!notice?.focus || !button) return;
    notice.focus = false;
    button.focus();
    // Focus given here is not the reader holding the notice, so it still leaves on time.
    release();
  });
</script>

<!-- Only there with a message: screens keep their own single `status` otherwise. -->
{#if notices.current}
  {@const notice = notices.current}
  <div
    class="notice-region"
    role="status"
    transition:appear
    onpointerenter={hold}
    onpointerleave={release}
    onfocusin={hold}
    onfocusout={release}
  >
    <div class="toast">
      <span>{notice.text}</span>
      {#if notice.undo}
        <button bind:this={button} class="button secondary" type="button" onclick={() => undo(notice)}>
          undo
        </button>
      {/if}
    </div>
  </div>
{/if}
