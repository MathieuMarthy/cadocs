<script lang="ts" module>
  /** A public link of a page (API.md, Sharing). */
  export interface PageShare {
    id: string;
    page_id: string;
    token: string;
    include_children: boolean;
    indexable: boolean;
    created_by: string | null;
    expires_at: number | null;
    created_at: number;
  }

  /** `YYYY-MM-DD` of a local date, as a date field reads and writes it. */
  export function dayOf(date: Date) {
    const pad = (n: number) => String(n).padStart(2, "0");
    return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
  }

  /** A link valid until a day works through that day: it expires as the next one starts, locally. */
  export function endOfDay(day: string) {
    const [year, month, date] = day.split("-").map(Number);
    return new Date(year, month - 1, date + 1).getTime();
  }
</script>

<script lang="ts">
  import { onMount, tick } from "svelte";

  import { api, ApiError } from "../lib/api";
  import { Form } from "../lib/form.svelte";
  import { formatLocale, t } from "../lib/i18n";
  import { collect } from "../lib/nav.svelte";
  import { exact } from "../lib/time";
  import Confirm from "./Confirm.svelte";
  import Field from "./Field.svelte";
  import IconButton from "./IconButton.svelte";

  /** The share dialog of a page (step 46): its public links, a new one, copy and revoke. */
  let { page, onclose }: { page: string; onclose: () => void } = $props();

  const headingId = $props.id();
  let dialog = $state<HTMLDialogElement>();
  let shares = $state<PageShare[] | null>(null);
  /** What the last action did, said in the dialog: the notice sits behind it, inert. */
  let said = $state("");
  /** Links revoked here, which a list read before the revoke must not bring back. */
  const revoked = new Set<string>();

  /** Says `text`, emptied first so the same result said twice is heard twice. */
  async function say(text: string) {
    said = "";
    await tick();
    said = text;
  }

  let includeChildren = $state(false);
  let indexable = $state(false);
  let until = $state("");
  const creating = new Form();
  const listing = new Form();
  const revoking = new Form();

  const publicUrl = (share: PageShare) => new URL(`/s/${share.token}`, location.origin).href;
  /** Read when the dialog opens: the field offers no day before it. */
  const today = dayOf(new Date());

  onMount(() => {
    dialog?.showModal();
    void listing.run(async () => {
      const { items } = await collect<PageShare>(`/pages/${page}/shares`, 1000);
      // A link created while the list was on its way stays, once.
      const made = (shares ?? []).filter((share) => !items.some((item) => item.id === share.id));
      shares = [...items.filter((item) => !revoked.has(item.id)), ...made];
    });
  });

  async function create(event: SubmitEvent) {
    event.preventDefault();
    const body = {
      include_children: includeChildren,
      indexable,
      expires_at: until ? endOfDay(until) : null,
    };
    const done = await creating.run(async () => {
      const share = await api<PageShare>("POST", `/pages/${page}/shares`, body);
      shares = [...(shares ?? []), share];
    });
    if (!done) return;
    void say(t("share.created"));
    includeChildren = false;
    indexable = false;
    until = "";
  }

  async function revoke(share: PageShare) {
    const done = await revoking.run(async () => {
      try {
        await api("DELETE", `/shares/${share.id}`);
      } catch (error) {
        // Revoked already, from another tab: it is gone either way.
        if (!(error instanceof ApiError && error.status === 404)) throw error;
      }
    });
    if (!done) return;
    revoked.add(share.id);
    shares = (shares ?? []).filter((item) => item.id !== share.id);
    void say(t("share.revoked"));
  }

  async function copyLink(share: PageShare) {
    try {
      await navigator.clipboard.writeText(publicUrl(share));
      void say(t("copy.link"));
    } catch {
      void say(t("copy.linkFailed"));
    }
  }

  const expired = (share: PageShare) => share.expires_at !== null && share.expires_at <= Date.now();
  /** The last day a link works, as the field picked it: the millisecond before it expires. */
  const lastDay = (expiresAt: number) => new Date(expiresAt - 1).toLocaleDateString(formatLocale());
</script>

<dialog bind:this={dialog} class="dialog share-dialog" aria-labelledby={headingId} onclose={onclose}>
  <div class="dialog-head">
    <h2 id={headingId} class="label">{t("share.title")}</h2>
    <IconButton icon="x" verb={t("share.close")} onclick={() => dialog?.close()} />
  </div>
  <p class="meta">{t("share.intro")}</p>

  {#if listing.problem}
    <p class="problem" role="alert">{listing.problem}</p>
  {:else if shares === null}
    <p class="meta">{t("common.loading")}</p>
  {:else if shares.length === 0}
    <p class="meta">{t("share.none")}</p>
  {:else}
    <ul class="share-list" aria-label={t("share.title")}>
      {#each shares as share (share.id)}
        <li class="share-row" class:expired={expired(share)}>
          <div class="share-link">
            <code>{publicUrl(share)}</code>
            <IconButton icon="copy" verb={t("share.copy")} label={t("share.copyLink")} onclick={() => copyLink(share)} />
          </div>
          <div class="share-foot">
            <!-- Facts, not a list: the rows are the list's items. -->
            <p class="meta share-facts">
              <span>{share.include_children ? t("share.children") : t("share.pageOnly")}</span>
              <span>{share.indexable ? t("share.indexed") : t("share.hidden")}</span>
              {#if share.expires_at === null}
                <span>{t("share.noExpiry")}</span>
              {:else if expired(share)}
                <span class="share-expired">{t("share.expired")}</span>
              {:else}
                <span title={exact(new Date(share.expires_at))}>{t("share.until", { date: lastDay(share.expires_at) })}</span>
              {/if}
            </p>
            <div class="actions">
              <Confirm
                label={t("share.revoke")}
                confirm={t("share.confirmRevoke")}
                danger
                disabled={revoking.busy}
                onconfirm={() => void revoke(share)}
              />
            </div>
          </div>
        </li>
      {/each}
    </ul>
  {/if}
  {#if revoking.problem}<p class="problem" role="alert">{revoking.problem}</p>{/if}

  <form class="share-new" onsubmit={create} aria-label={t("share.new")}>
    <h3 class="label">{t("share.new")}</h3>
    <label class="check">
      <input type="checkbox" bind:checked={includeChildren} />
      <span>{t("share.includeChildren")}</span>
    </label>
    <label class="check">
      <input type="checkbox" bind:checked={indexable} />
      <span>{t("share.letIndex")}</span>
    </label>
    <Field
      label={t("share.untilField")}
      type="date"
      min={today}
      bind:value={until}
      problem={creating.field("expires_at")}
    />
    {#if creating.problem}<p class="problem" role="alert">{creating.problem}</p>{/if}
    <div class="actions">
      <button class="button" type="submit" disabled={creating.busy}>{t("share.create")}</button>
    </div>
  </form>
  <p class="meta share-said" role="status">{said}</p>
</dialog>
