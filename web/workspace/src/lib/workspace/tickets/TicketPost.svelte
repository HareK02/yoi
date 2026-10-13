<script lang="ts">
  import type { TicketEventDetail } from "#lib/generated/ticket-api.ts";
  import RichMarkdown from "#lib/workspace/console/RichMarkdown.svelte";
  import { relationLabel } from "./ticket-panel.ts";

  const { post, busy, load, save } = $props<{
    post: TicketEventDetail;
    busy: boolean;
    load: (post: TicketEventDetail) => Promise<TicketEventDetail>;
    save: (post: TicketEventDetail, body: string) => Promise<void>;
  }>();
  let editing = $state(false);
  let loading = $state(false);
  let saving = $state(false);
  let observed = $state<TicketEventDetail | null>(null);
  let fullPost = $state<TicketEventDetail | null>(null);
  let fullPostSourceDigest = $state<string | null>(null);
  let draft = $state("");
  let error = $state<string | null>(null);
  let reconcile = $state(false);
  const displayed = $derived(fullPost && fullPostSourceDigest === post.body_digest ? fullPost : post);
  const editable = $derived(post.editable && post.kind === "comment");

  function prettyDate(value?: string | null): string {
    if (!value) return "Unknown time";
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
  }

  async function readFull(edit: boolean): Promise<void> {
    if (busy || loading || saving) return;
    const sourceDigest = post.body_digest;
    loading = true;
    error = null;
    try {
      const current = await load(post);
      if (current.event_ref !== post.event_ref || current.body_truncated) {
        throw new Error("The full post could not be loaded. No partial text can be saved.");
      }
      fullPost = current;
      fullPostSourceDigest = sourceDigest;
      if (edit) {
        if (!current.editable || current.kind !== "comment") throw new Error("This event is not an editable post.");
        observed = current;
        draft = current.body ?? "";
        editing = true;
        reconcile = false;
      }
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      loading = false;
    }
  }

  function beginEdit(): void {
    if (post.body_truncated) { void readFull(true); return; }
    observed = displayed;
    draft = displayed.body ?? "";
    error = null;
    reconcile = false;
    editing = true;
  }

  async function savePost(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (busy || saving || !observed || observed.body_truncated || reconcile) return;
    saving = true;
    error = null;
    try {
      await save(observed, draft);
      editing = false;
      fullPost = null;
      observed = null;
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      error = /\(409\)/.test(message)
        ? `Post update conflict. Your draft is preserved. Load the current post before editing again. ${message}`
        : `Post save was not confirmed. Your draft is preserved; read the current post before retrying. ${message}`;
      reconcile = true;
    } finally {
      saving = false;
    }
  }
</script>

<article class="ticket-thread-event" data-initial={post.is_initial} aria-label={post.is_initial ? "Initial post" : post.kind === "comment" ? "Discussion post" : relationLabel(post.kind)}>
  <header>
    <div class="ticket-post-attribution">
      <strong>{displayed.author ?? "Unknown author"}</strong>
      {#if displayed.kind !== "comment"}<span>{displayed.heading ?? relationLabel(displayed.kind)}</span>{/if}
      <time datetime={displayed.at ?? undefined}>{prettyDate(displayed.at)}</time>
      {#if displayed.edited_at}
        <span class="ticket-post-edited">Edited <time datetime={displayed.edited_at}>{prettyDate(displayed.edited_at)}</time> by {displayed.edited_by ?? "Unknown editor"}</span>
      {/if}
    </div>
    {#if editable && !editing}
      <button class="workspace-secondary-button" type="button" disabled={busy || loading} onclick={beginEdit}>{loading ? "Loading post…" : "Edit post"}</button>
    {/if}
  </header>
  {#if editing}
    <form class="ticket-editor" aria-label="Edit post" onsubmit={savePost}>
      <label>Post body<textarea bind:value={draft} rows="8" disabled={saving}></textarea></label>
      <div class="ticket-comment-actions">
        <button class="workspace-primary-button" type="submit" disabled={busy || saving || reconcile}>{saving ? "Saving…" : "Save post"}</button>
        <button class="workspace-secondary-button" type="button" disabled={saving} onclick={() => { editing = false; error = null; }}>Cancel post edit</button>
      </div>
      {#if reconcile}<button class="workspace-secondary-button" type="button" disabled={busy || loading} onclick={() => void readFull(true)}>Discard draft and load current post</button>{/if}
    </form>
  {:else}
    {#if displayed.from || displayed.to}<p>{displayed.from ?? "—"} → {displayed.to ?? "—"}</p>{/if}
    {#if displayed.reason && displayed.reason !== displayed.body}<RichMarkdown text={displayed.reason} />{/if}
    {#if displayed.body}<RichMarkdown text={displayed.body} />{:else if displayed.kind === "comment"}<p class="workspace-empty-copy">Empty post.</p>{/if}
    {#each displayed.references as reference}<p>{reference}</p>{/each}
    {#if displayed.body_truncated}
      <p class="workspace-empty-copy">This post is shortened.</p>
      <button class="workspace-secondary-button" type="button" disabled={busy || loading} onclick={() => void readFull(false)}>{loading ? "Loading post…" : "Read full post"}</button>
    {/if}
  {/if}
  {#if error}<p class="workspace-callout is-error" role="alert">{error}</p>{/if}
</article>
