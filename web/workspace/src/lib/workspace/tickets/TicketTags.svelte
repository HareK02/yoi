<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import type { TicketDetail, WorkspaceTag, WorkspaceTagMutation, TicketTagSetRequest } from "#lib/generated/ticket-api.ts";
  import { workspaceApiJson, workspaceApiJsonWithBody, workspaceApiPath } from "#lib/workspace/api/http.ts";
  import { parseWorkspaceTag, TICKET_BROWSER_API_MAX_RESPONSE_BYTES } from "#lib/workspace/api/ticket-browser.ts";
  import { observeOperation, operationError, parseTags, type ObservedOperation } from "#lib/workspace/api/ticket-operations.ts";
  let { workspaceId, ticket, refresh }: { workspaceId: string; ticket: TicketDetail; refresh: () => Promise<void> } = $props();
  let catalog = $state<WorkspaceTag[]>([]);
  let selected = $state<string[]>([]);
  let loading = $state(true);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let pending = $state<ObservedOperation | null>(null);
  let saved = $state(false);
  let name = $state("");
  let tagId = $state("");
  const observed = $derived(catalog.find((tag) => tag.id === tagId));
  let alive = true;
  onDestroy(() => { alive = false; });
  const catalogPath = $derived(workspaceApiPath(workspaceId, "/ticket-tags"));
  async function load() {
    loading = true;
    try {
      catalog = await workspaceApiJson(catalogPath, parseTags, TICKET_BROWSER_API_MAX_RESPONSE_BYTES);
      selected = ticket.tags.map((tag) => tag.id);
      if (!pending) error = null;
      return true;
    } catch (cause) { if (alive) error = operationError(cause); return false; }
    finally { loading = false; }
  }
  onMount(() => { void load(); });
  async function run(operation: ObservedOperation, tagResult: boolean) {
    if (busy) return;
    busy = true; pending = operation; saved = false; error = null;
    try {
      await workspaceApiJsonWithBody<WorkspaceTag | WorkspaceTag[]>(operation.path, { method: operation.method, body: operation.body }, tagResult ? parseWorkspaceTag : parseTags, TICKET_BROWSER_API_MAX_RESPONSE_BYTES);
      if (!alive) return;
      pending = null; saved = true;
      await refresh(); await load();
    } catch (cause) { if (alive) error = saved ? `Tag operation saved; latest projection unavailable. ${operationError(cause)}` : operationError(cause); }
    finally { busy = false; }
  }
  let retryTagResult = $state(false);
  function mutateTag(action: WorkspaceTagMutation["action"]) {
    retryTagResult = true;
    void run(observeOperation(catalogPath, "POST", { action, operation_key: crypto.randomUUID() } satisfies WorkspaceTagMutation), true);
  }
  function setTags() {
    retryTagResult = false;
    const request: TicketTagSetRequest = { expected_tag_ids: ticket.tags.map((tag) => tag.id), tag_ids: selected, operation_key: crypto.randomUUID() };
    void run(observeOperation(workspaceApiPath(workspaceId, `/tickets/${encodeURIComponent(ticket.id)}/tags`), "PUT", request), false);
  }
  async function reconcile() {
    if (busy) return;
    busy = true;
    try { await refresh(); if (!alive || !await load()) return; pending = null; error = null; saved = false; }
    catch (cause) { if (alive) error = operationError(cause); }
    finally { busy = false; }
  }
</script>
<section class="ticket-detail-section ticket-tags">
  <div class="ticket-section-heading"><h2>Classification tags</h2></div>
  <p class="workspace-empty-copy">Classification only. Tag changes do not close or reopen this case.</p>
  <div class="ticket-tag-chips">{#each ticket.tags as tag (tag.id)}<span class="ticket-tag-chip">{tag.name}</span>{:else}<span>No tags</span>{/each}</div>
  {#if loading}<p role="status">Loading tag catalog…</p>{/if}
  {#if error}<p class="workspace-callout is-error" role="alert">{error}</p>{/if}
  {#if saved}<p role="status">Tag operation saved.</p>{/if}
  {#if pending}
    <p>Pending operation retained; new operations are disabled.</p>
    <button class="workspace-secondary-button" disabled={busy} onclick={() => pending && void run(pending, retryTagResult)}>Retry exact tag operation</button>
    <button class="workspace-secondary-button" disabled={busy} onclick={() => void reconcile()}>Refresh and reconcile tags</button>
  {/if}
  <details><summary>Edit classification</summary>
    <form class="ticket-control-form" onsubmit={(event) => { event.preventDefault(); setTags(); }}>
      <fieldset disabled={busy || loading || pending !== null}><legend>Multiple tags</legend>
        {#each catalog.filter((tag) => !tag.deleted) as tag (tag.id)}<label><input type="checkbox" bind:group={selected} value={tag.id} />{tag.name}</label>{/each}
      </fieldset>
      <button class="workspace-secondary-button" disabled={busy || loading || pending !== null}>Save classification</button>
    </form>
  </details>
  <details><summary>Manage Workspace tags</summary>
    <form class="ticket-control-form" onsubmit={(event) => { event.preventDefault(); mutateTag({ kind: "create", name: name.trim() }); }}>
      <label>Tag name<input bind:value={name} required disabled={busy || pending !== null} /></label>
      <button class="workspace-secondary-button" disabled={busy || pending !== null || !name.trim()}>Create tag</button>
    </form>
    <label>Existing tag<select bind:value={tagId} disabled={busy || pending !== null}><option value="">Select tag</option>{#each catalog.filter((tag) => !tag.deleted) as tag (tag.id)}<option value={tag.id}>{tag.name}</option>{/each}</select></label>
    {#if observed}
      <p><code>{observed.id}</code></p>
      <button class="workspace-secondary-button" disabled={busy || pending !== null || !name.trim()} onclick={() => mutateTag({ kind: "rename", tag_id: observed.id, expected_last_operation_key: observed.last_operation_key, name: name.trim() })}>Rename tag</button>
      <button class="workspace-danger-button" disabled={busy || pending !== null} onclick={() => { if (confirm(`Delete classification tag ${observed.name}? Historical audit is retained.`)) mutateTag({ kind: "delete", tag_id: observed.id, expected_last_operation_key: observed.last_operation_key }); }}>Delete tag</button>
    {/if}
  </details>
</section>
<style>
  .ticket-tag-chips { display: flex; flex-wrap: wrap; gap: var(--space-3); margin-block: var(--space-3); }
  .ticket-tag-chip { overflow-wrap: anywhere; }
  fieldset { min-width: 0; border: 0; padding: 0; margin: var(--space-3) 0; }
  fieldset label { display: flex; align-items: center; gap: 0.5rem; }
  fieldset input { width: auto; }
  code { overflow-wrap: anywhere; }
</style>
