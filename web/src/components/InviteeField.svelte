<script lang="ts">
  // The email field of the invitation form (step 74): accounts of the instance show below it once
  // 3 characters are typed, in the command menu's style, and any address still goes.
  import { untrack } from "svelte";

  import { api } from "../lib/api";
  import { t } from "../lib/i18n";

  interface Invitee {
    id: string;
    name: string;
    email: string;
  }

  let {
    workspace,
    value = $bindable(""),
    problem = null,
  }: { workspace: string; value?: string; problem?: string | null } = $props();

  const id = $props.id();
  let found = $state<Invitee[]>([]);
  /** The suggestion Enter picks, none until an arrow moves to one, so Enter still submits. */
  let active = $state(-1);
  /** Only the last search shows, and a pick drops the one under way. */
  let asked = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;

  function close() {
    clearTimeout(timer);
    asked += 1;
    found = [];
    active = -1;
  }

  function typed() {
    close();
    const q = value.trim();
    // The server's bounds: shorter is a `400`, and an address is never longer.
    if (q.length < 3 || q.length > 200) return;
    const ask = asked;
    // A pause in typing, so a name typed fast costs one search of the admin's budget.
    timer = setTimeout(async () => {
      const query = new URLSearchParams({ q });
      try {
        const path = `/workspaces/${workspace}/invitees?${query}`;
        const { items } = await api<{ items: Invitee[] }>("GET", path);
        if (ask === asked) found = items;
      } catch {
        // Suggestions only help: a refused search (too many, a lost role) shows none.
      }
    }, 200);
  }

  // A form that sent the address clears it without an `input` event.
  $effect(() => {
    if (value === "") untrack(close);
  });

  function pick(person: Invitee) {
    value = person.email;
    close();
  }

  function key(event: KeyboardEvent) {
    if (found.length === 0) return;
    if (event.key === "ArrowDown") {
      event.preventDefault();
      active = (active + 1) % found.length;
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      active = active <= 0 ? found.length - 1 : active - 1;
    } else if (event.key === "Enter" && active >= 0) {
      event.preventDefault();
      pick(found[active]!);
    } else if (event.key === "Escape") {
      event.preventDefault();
      close();
    }
  }
</script>

<div class="field invitee">
  <label class="label" for="{id}-input">{t("form.email")}</label>
  <input
    id="{id}-input"
    type="email"
    required
    autocomplete="off"
    role="combobox"
    aria-autocomplete="list"
    aria-expanded={found.length > 0}
    aria-controls={found.length > 0 ? `${id}-list` : undefined}
    aria-activedescendant={active >= 0 && found.length > 0 ? `${id}-${active}` : undefined}
    aria-invalid={problem ? "true" : undefined}
    bind:value
    oninput={typed}
    onkeydown={key}
    onblur={close}
  />
  {#if found.length > 0}
    <!-- A press on a suggestion must not blur the field first, which would close the list. -->
    <div
      class="command-menu"
      id="{id}-list"
      role="listbox"
      tabindex="-1"
      aria-label={t("workspace.suggestions")}
      onmousedown={(event) => event.preventDefault()}
    >
      {#each found as person, index (person.id)}
        <!-- svelte-ignore a11y_click_events_have_key_events: the field's arrows and Enter pick -->
        <div
          class="command-menu-item"
          id="{id}-{index}"
          role="option"
          tabindex="-1"
          aria-selected={index === active}
          onclick={() => pick(person)}
          onmousemove={() => (active = index)}
        >
          <span>{person.name}</span>
          <span class="command-menu-hint">{person.email}</span>
        </div>
      {/each}
    </div>
  {/if}
  {#if problem}<span class="problem" role="alert">{problem}</span>{/if}
</div>
