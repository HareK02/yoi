<script lang="ts">
  import { mergeRequestPagePath } from "$lib/workspace/api/merge-requests";
  import type { PageData } from "./$types";

  let { data }: { data: PageData } = $props();

  function prettyDate(value: string): string {
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
  }
</script>

<svelte:head><title>Merge Requests · Yoi</title></svelte:head>

<div class="workspace-page">
  <header class="workspace-page-header">
    <div>
      <p class="workspace-eyebrow">Workspace resources</p>
      <h1>Merge Requests</h1>
    </div>
    <span class="workspace-count">{data.mergeRequests?.items.length ?? 0}</span>
  </header>

  {#if data.error}
    <p class="workspace-callout is-error">{data.error}</p>
  {:else}
    <div class="ticket-list" aria-label="Merge Requests">
      {#each data.mergeRequests?.items ?? [] as item (item.summary.merge_request_id)}
        {@const mergeRequest = item.summary}
        <a
          class="ticket-row"
          href={mergeRequestPagePath(data.workspaceId, mergeRequest.merge_request_id)}
        >
          <div class="ticket-main">
            <div class="ticket-title-row">
              <span class="ticket-key">{mergeRequest.merge_request_id}</span>
              <strong class="ticket-title">
                {mergeRequest.selector_from ?? "Source selector requires repair"}
                → {mergeRequest.selector_to}
              </strong>
            </div>
            <p class="ticket-summary">
              Repository {mergeRequest.repository_id} · {item.ticket_ids.length} linked Ticket{item.ticket_ids.length === 1 ? "" : "s"} · review {mergeRequest.review_status}
            </p>
          </div>
          <div class="ticket-meta">
            <span class={`ticket-state state-${mergeRequest.state}`}>{mergeRequest.state}</span>
            <time>{prettyDate(mergeRequest.updated_at)}</time>
          </div>
        </a>
      {:else}
        <p class="workspace-empty-copy">No Merge Requests.</p>
      {/each}
    </div>
  {/if}
</div>
