<script lang="ts">
  import { mergeRequestPagePath } from "$lib/workspace/api/merge-requests";
  import type { PageData } from "./$types";

  let { data }: { data: PageData } = $props();

  function prettyDate(value: string): string {
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
  }

  function textField(event: Record<string, unknown>, key: string): string | null {
    const value = event[key];
    return typeof value === "string" && value.length > 0 ? value : null;
  }
</script>

<svelte:head><title>Merge Request · Yoi</title></svelte:head>

<div class="workspace-page">
  <header class="workspace-page-header">
    <div>
      <p class="workspace-eyebrow">Merge Request</p>
      <h1>{data.mergeRequest?.merge_request_id ?? data.mergeRequestId}</h1>
      {#if data.mergeRequest}
        <p class="workspace-page-lede">
          {data.mergeRequest.selector_from ?? "Source selector requires repair"}
          → {data.mergeRequest.selector_to}
        </p>
      {/if}
    </div>
    <a class="workspace-secondary-button" href={mergeRequestPagePath(data.workspaceId)}>
      All Merge Requests
    </a>
  </header>

  {#if data.error}
    <p class="workspace-callout is-error">{data.error}</p>
  {:else if data.mergeRequest}
    {@const mergeRequest = data.mergeRequest}
    <div class="ticket-detail-grid">
      <main class="ticket-detail-main">
        <section class="ticket-detail-section">
          <div class="ticket-section-heading"><h2>Selectors</h2></div>
          <dl class="ticket-facts">
            <div><dt>Repository</dt><dd>{mergeRequest.repository_id}</dd></div>
            <div><dt>State</dt><dd>{mergeRequest.state}</dd></div>
            <div><dt>Source selector</dt><dd><code>{mergeRequest.selector_from ?? "requires repair"}</code></dd></div>
            <div><dt>Source revision</dt><dd>{mergeRequest.source.status}{mergeRequest.source.ref ? ` · ${mergeRequest.source.ref}` : ""}</dd></div>
            <div><dt>Target selector</dt><dd><code>{mergeRequest.selector_to}</code></dd></div>
            <div><dt>Target revision</dt><dd>{mergeRequest.target.status}{mergeRequest.target.ref ? ` · ${mergeRequest.target.ref}` : ""}</dd></div>
            <div><dt>Updated</dt><dd>{prettyDate(mergeRequest.updated_at)}</dd></div>
          </dl>
        </section>

        <section class="ticket-detail-section">
          <div class="ticket-section-heading">
            <h2>Thread</h2><span>{mergeRequest.thread.length}</span>
          </div>
          <div class="ticket-timeline">
            {#each mergeRequest.thread as event (event.sequence)}
              <article>
                <div class="ticket-timeline-marker"></div>
                <div>
                  <header>
                    <strong>{event.kind}</strong>
                    <time>{prettyDate(event.at)}</time>
                  </header>
                  {#if textField(event, "subject_ref")}
                    <p><code>{textField(event, "subject_ref")}</code></p>
                  {/if}
                  {#if textField(event, "decision")}<p>{textField(event, "decision")}</p>{/if}
                  {#if textField(event, "reason")}<p>{textField(event, "reason")}</p>{/if}
                  {#if textField(event, "body")}<p>{textField(event, "body")}</p>{/if}
                </div>
              </article>
            {:else}
              <p class="workspace-empty-copy">No Merge Request events.</p>
            {/each}
          </div>
        </section>
      </main>

      <aside class="ticket-control-rail">
        <section class="ticket-control-card">
          <header><h2>Linked Tickets</h2></header>
          {#each mergeRequest.linked_tickets as linkedTicket}
            {#if linkedTicket.key}
              <p>
                <a href={`/w/${encodeURIComponent(data.workspaceId)}/tickets/${encodeURIComponent(linkedTicket.key)}`}>
                  {linkedTicket.key}
                </a>
              </p>
            {:else}
              <p class="workspace-empty-copy">Linked Ticket key unavailable.</p>
            {/if}
          {:else}
            <p class="workspace-empty-copy">No linked Tickets.</p>
          {/each}
        </section>
      </aside>
    </div>
  {/if}
</div>
