<script lang="ts">
  import { goto } from "$app/navigation";
  import { untrack } from "svelte";
  import { loadJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
  import { parseTicketQueryResponse, TICKET_BROWSER_API_LOAD_POLICY } from "#lib/workspace/api/ticket-browser.ts";
  import { currentOrchestratorAssignment, handlingLabel, handlingSummary, ticketQueryRequest, closureReasonLabel, assignmentPrincipalLabel } from "#lib/workspace/tickets/ticket-panel.ts";
  import { ticketHref } from "#lib/workspace/resource-links.ts";
  import type { PageData as RouteData } from "./$types";
  import type { ResolvedPageData } from "#lib/workspace/navigation/deferred.ts";

  type PageData = ResolvedPageData<RouteData>;
  let { data }: { data: PageData } = $props();
  let response = $state(untrack(() => data.tickets));
  let busy = $state(false);
  $effect(() => { const latest = data.tickets; untrack(() => { response = latest; }); });
  async function applyFilters(event: SubmitEvent) {
    event.preventDefault();
    const search = new URLSearchParams();
    for (const [key, value] of new FormData(event.currentTarget as HTMLFormElement)) if (typeof value === "string" && value) search.append(key, value);
    busy = true;
    try { await goto(`?${search}`); } finally { busy = false; }
  }
  async function loadMore() {
    if (busy || (response.data && !response.data.page.has_more)) return;
    busy = true;
    const observedData = data;
    const previous = response.data;
    const result = await loadJson(fetch, workspaceApiPath(data.workspaceId, "/tickets/query"), {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify(ticketQueryRequest(data.filters, data.filters.caseState === "all" ? [] : [data.filters.caseState], previous?.page.next_cursor ?? null, data.handling)),
    }, parseTicketQueryResponse, TICKET_BROWSER_API_LOAD_POLICY);
    if (data !== observedData) { busy = false; return; }
    if (result.data && previous) {
      const items = new Map(previous.items.map((item) => [item.id, item]));
      for (const item of result.data.items) items.set(item.id, item);
      response = { data: { ...result.data, items: [...items.values()] }, error: null };
    } else response = { data: result.data ?? previous, error: result.error };
    busy = false;
  }
</script>

<svelte:head><title>Subject handling · {data.workspaceId}</title></svelte:head>
<div class="workspace-page orchestrator-page">
  <header class="workspace-page-header"><h1>Subject handling</h1><a href={`/w/${encodeURIComponent(data.workspaceId)}/tickets`}>Ticket cases</a></header>
  <p>Responsibility belongs to each Subject, independently of its current Worker.</p>
  <form class="orchestrator-filters" aria-label="Subject handling filters" onsubmit={applyFilters}>
    <input type="hidden" name="principal" value={data.filters.subjectId ? "subject" : "all"} />
    <label>Subject identity<input name="subject" value={data.filters.subjectId} placeholder="All Subjects" oninput={(event) => { const input = event.currentTarget.form?.elements.namedItem("principal") as HTMLInputElement | null; if (input) input.value = event.currentTarget.value.trim() ? "subject" : "all"; }} /></label>
    <label>Search<input name="q" type="search" value={data.filters.query} /></label>
    <label>Handling (self-reported)<select name="handling" value={data.handling ?? ""}><option value="">All including unknown</option><option value="waiting">Waiting</option><option value="inprogress">In progress</option><option value="done">Done</option></select></label>
    <label>Ticket case<select name="case" value={data.filters.caseState}><option value="all">Open and closed</option><option value="open">Open</option><option value="closed">Closed</option></select></label>
    <button class="workspace-secondary-button" disabled={busy}>{busy ? "Loading…" : "Apply filters"}</button>
  </form>
  <section class="orchestrator-assignments" aria-label="Current Subject assignments">
    {#each response.data?.items ?? [] as ticket (ticket.id)}
      {@const row = currentOrchestratorAssignment(ticket)}
      <a class="orchestrator-assignment" href={ticketHref(data.workspaceId, ticket)}>
        <div class="assignment-heading"><span>{ticket.resource_key}</span><strong>{ticket.title}</strong></div>
        <div class="assignment-meta"><span>Handling: {handlingLabel(row?.tracking?.status)} ({row?.tracking ? "self-reported" : "not reported"})</span><span>Ticket case: {ticket.state}</span></div>
        {#if row}<p>{assignmentPrincipalLabel(row.principal)}</p>{/if}
        <p>{handlingSummary(row)}</p>
        {#if row?.tracking?.next_action}<p>Next action: {row.tracking.next_action}</p>{/if}
        {#if ticket.current_closure}<p>Close reason: {closureReasonLabel(ticket.current_closure.reason)}</p>{/if}
        <span>Tags: {ticket.tags.length ? ticket.tags.map((tag) => tag.name).join(", ") : "None"}</span>
        {#if row?.updated_at}<time datetime={row.updated_at}>Handling updated: {new Date(row.updated_at).toLocaleString()}</time>{/if}
        <span class="assignment-detail-link">View Ticket, handling history and delivery evidence →</span>
      </a>
    {:else}
      {#if !response.error}<p class="workspace-empty-copy">No current assignments match these filters.</p>{/if}
    {/each}
    {#if response.error}<div class="workspace-callout is-error" role="alert"><p>{response.error}</p><button class="workspace-secondary-button" type="button" onclick={loadMore} disabled={busy}>Retry query</button></div>{/if}
    {#if busy}<p aria-live="polite">Loading assignments…</p>{:else if response.data?.page.has_more && !response.error}<button class="workspace-secondary-button" type="button" onclick={loadMore}>Load more assignments</button>{/if}
  </section>
</div>

<style>
  .orchestrator-page { max-width: 72rem; }
  .workspace-page-header { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: var(--space-3); }
  .orchestrator-filters { display: flex; flex-wrap: wrap; align-items: end; gap: var(--space-3); margin-block: var(--space-4); }
  .orchestrator-filters label { display: grid; gap: var(--space-1); min-width: 0; flex: 1 1 12rem; }
  input, select { min-width: 0; width: 100%; padding: var(--space-2); color: var(--text); background: var(--bg-raised); border: 1px solid var(--line); border-radius: var(--radius-soft); font: inherit; }
  .orchestrator-assignments { display: grid; }
  .orchestrator-assignment { display: grid; gap: var(--space-2); padding-block: var(--space-4); border-bottom: 1px solid var(--line); color: var(--text); text-decoration: none; overflow-wrap: anywhere; }
  .orchestrator-assignment:hover { background: var(--interactive-hover); }
  .assignment-heading, .assignment-meta { display: flex; flex-wrap: wrap; gap: var(--space-3); }
  .assignment-heading strong { color: var(--text-strong); }
  .assignment-meta, time { color: var(--text-muted); font-size: var(--font-size-compact); }
  .orchestrator-assignment p { margin: 0; white-space: pre-wrap; }
  .assignment-detail-link { color: var(--accent); }
</style>
