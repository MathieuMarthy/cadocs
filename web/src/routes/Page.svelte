<script lang="ts">
  import { onMount } from "svelte";

  import Source from "../components/Source.svelte";
  import { openPage } from "../editor/collab";
  import { api, ApiError } from "../lib/api";
  import { sessionEnded } from "../lib/session.svelte";
  import Message from "./Message.svelte";

  let { id }: { id: string } = $props();

  /** Why this page cannot show. A failed session goes to login instead. */
  let gone = $state<string | null>(null);

  let editor = $state<HTMLElement>()!;
  let title = $state<HTMLInputElement>()!;
  let status = $state<HTMLElement>()!;
  let presence = $state<HTMLElement>()!;
  let name = $state("");

  /** A socket cannot read the status of a failed upgrade, so REST says what went wrong. */
  async function check() {
    try {
      await api("GET", `/pages/${id}`);
    } catch (error) {
      if (error instanceof ApiError && error.status === 401) return false;
      if (error instanceof ApiError && error.status === 404) {
        gone = "page not found";
        return false;
      }
      // Anything else is the network or a passing failure, which the socket reports and outlives.
    }
    return true;
  }

  onMount(() => {
    let page: ReturnType<typeof openPage> | null = null;
    let left = false;
    void check().then((readable) => {
      if (!readable || left) return;
      page = openPage(id, { editor, title, status, presence }, {
        onName: (text) => (name = text),
        onFinalClose(code) {
          page?.destroy();
          page = null;
          if (code === 4401) sessionEnded();
          else gone = code === 4403 ? "access revoked" : "page deleted";
        },
        onRepeatedFailure() {
          void check().then((readable) => {
            if (readable) return;
            page?.destroy();
            page = null;
          });
        },
      });
    });
    return () => {
      left = true;
      page?.destroy();
    };
  });
</script>

{#if gone}
  <Message title={gone} />
{:else}
  <header class="topbar">
    <a class="logo" href="/" aria-label="cadocs"></a>
    <nav class="breadcrumb" aria-label="location">
      <a href="/">cadocs</a>
      <span class="sep">/</span>
      <span class="current">{name || id}</span>
    </nav>
    <div class="topbar-end">
      <span bind:this={status} class="status" role="status"></span>
      <div bind:this={presence} class="presence"></div>
    </div>
  </header>
  <main class="page">
    <input
      bind:this={title}
      class="title"
      type="text"
      placeholder="untitled"
      aria-label="page title"
      autocomplete="off"
    />
    <div bind:this={editor}></div>
  </main>
  <footer class="page-end"><Source /></footer>
{/if}
