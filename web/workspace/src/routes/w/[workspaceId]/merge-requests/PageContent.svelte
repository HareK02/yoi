<script lang="ts">
  import { mergeRequestPagePath, mergeRequestQueryPagePath } from "#lib/workspace/api/merge-requests.ts";
  import { summarySourceReviewStatus } from "#lib/workspace/merge-request-status.ts";
  import type { PageData as RouteData } from "./$types";
  import type { ResolvedPageData } from "#lib/workspace/navigation/deferred.ts";

  type PageData = ResolvedPageData<RouteData>;
  let { data }: { data: PageData } = $props();
  const query = $derived(new URLSearchParams(data.query));
  const phases = ["implementation", "integration", "completed", "canceled"];

  function prettyDate(value: string): string {
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
  }
</script>

<svelte:head><title>Merge Requests · Yoi</title></svelte:head>

<div class="workspace-page">
  <header class="workspace-page-header">
    <h1>Merge Requests</h1>
    {#if data.mergeRequests}<span class="workspace-count">{data.mergeRequests.items.length} on this page</span>{/if}
  </header>

  {#key data.query}
    <form method="GET" action={mergeRequestPagePath(data.workspaceId)} class="mr-search" role="search" aria-label="Merge Requests">
      <div class="mr-query-fields">
        <label>Repository<input name="repository_key" value={query.get("repository_key") ?? ""} /></label>
        <label>Target<input name="selector_to" value={query.get("selector_to") ?? ""} placeholder="develop" /></label>
        <div class="mr-field"><label for="mr-phase">Phase</label>
          <select id="mr-phase" name="phase" value={query.get("phase") ?? ""}>
            <option value="">All phases</option>
            {#if query.get("phase") && !phases.includes(query.get("phase")!)}
              <option value={query.get("phase")!}>{query.get("phase")}</option>
            {/if}
            {#each phases as phase}<option value={phase}>{phase}</option>{/each}
          </select>
        </div>
      </div>
      <details open={!!(query.get("selector_from") || query.get("ticket_ref") || query.get("state") || query.get("assigned_to_self"))}>
        <summary>More conditions</summary>
        <div class="mr-query-fields">
          <label>Source<input name="selector_from" value={query.get("selector_from") ?? ""} /></label>
          <label>Ticket<input name="ticket_ref" value={query.get("ticket_ref") ?? ""} placeholder="T-123" /></label>
          <label>Page size<input name="limit" type="number" min="1" max="100" value={query.get("limit") ?? "50"} /></label>
        </div>
        {#each ["state", "assigned_to_self"] as key}
          {#if query.has(key)}
            <input type="hidden" name={key} value={query.get(key)!} />
            <p>{key}: {query.get(key)}</p>
          {/if}
        {/each}
      </details>
      <div class="mr-query-actions">
        <button class="workspace-secondary-button" type="submit">Search</button>
        <a class="workspace-secondary-button" href={mergeRequestPagePath(data.workspaceId)}>Clear</a>
      </div>
    </form>
  {/key}

  {#if data.error}
    <p class="workspace-callout is-error" role="alert">{data.error}</p>
  {:else}
    <div class="ticket-list" aria-label="Merge Requests">
      {#each data.mergeRequests?.items ?? [] as item (item.summary.merge_request_id)}
        {@const mergeRequest = item.summary}
        <a class="ticket-row" href={mergeRequestPagePath(data.workspaceId, mergeRequest.merge_request_id)}>
          <div class="ticket-main">
            <div class="ticket-title-row">
              <span class="ticket-key">{mergeRequest.merge_request_id}</span>
              <strong class="ticket-title">{mergeRequest.selector_from ?? "Source selector requires repair"} → {mergeRequest.selector_to}</strong>
            </div>
            <p class="ticket-summary">Repository {mergeRequest.repository_key} · {item.ticket_ids.length} linked Ticket{item.ticket_ids.length === 1 ? "" : "s"}</p>
            <p class="ticket-summary">{summarySourceReviewStatus(mergeRequest)}</p>
            <p class="ticket-summary">MR Worker: {item.worker_assignment?.worker ? (item.worker_resource_key ?? "Assigned — public reference unavailable") : "Unassigned"}</p>
            {#if mergeRequest.integration_evidence_error != null}
              <p class="workspace-callout is-error">Integration evidence error: {mergeRequest.integration_evidence_error}</p>
            {/if}
          </div>
          <div class="ticket-meta">
            <span class={`ticket-state state-${mergeRequest.state.kind}`}>Phase: {mergeRequest.state.kind}</span>
            <time>{prettyDate(mergeRequest.updated_at)}</time>
          </div>
        </a>
      {:else}
        <p class="workspace-empty-copy">No Merge Requests match these conditions.</p>
      {/each}
    </div>
  {/if}
  <nav class="mr-pages" aria-label="Merge Request pages">
    {#if query.has("cursor")}
      <a class="workspace-secondary-button" href={mergeRequestQueryPagePath(data.workspaceId, query, null)}>First page</a>
    {/if}
    {#if !data.error && data.mergeRequests?.next_cursor}
      <a class="workspace-secondary-button" href={mergeRequestQueryPagePath(data.workspaceId, query, data.mergeRequests.next_cursor)}>Next page</a>
    {/if}
  </nav>
</div>

<style>
  .workspace-page-header { margin-bottom: var(--space-4); }
  .ticket-key { flex-shrink: 0; white-space: nowrap; }
  .mr-search { display: grid; gap: var(--space-3); margin-bottom: var(--space-5); }
  .mr-query-fields { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 200px), 1fr)); gap: var(--space-3); }
  label, .mr-field { display: grid; gap: var(--space-1); min-width: 0; }
  input, select { box-sizing: border-box; width: 100%; min-width: 0; padding: var(--space-2); border: 1px solid var(--line); border-radius: var(--radius-soft); background: var(--bg); color: var(--text); font: inherit; }
  input:hover, select:hover { border-color: var(--line-strong); }
  input:focus-visible, select:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  details .mr-query-fields { margin-top: var(--space-3); }
  .mr-query-actions, .mr-pages { display: flex; gap: var(--space-2); flex-wrap: wrap; }
  .mr-pages { margin-top: var(--space-4); }
  .ticket-main, .ticket-title { min-width: 0; overflow-wrap: anywhere; }
</style>
