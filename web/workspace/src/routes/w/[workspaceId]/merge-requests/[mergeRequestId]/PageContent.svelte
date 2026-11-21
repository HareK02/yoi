<script lang="ts">
  import type { MergeRequestThreadEvent } from "#lib/generated/ticket-api.ts";
  import { mergeRequestPagePath } from "#lib/workspace/api/merge-requests.ts";
  import {
    sourceReviewFreshness,
    targetIntegrationStatus,
  } from "#lib/workspace/merge-request-status.ts";
  import type { PageData as RouteData } from "./$types";
  import type { ResolvedPageData } from "#lib/workspace/navigation/deferred.ts";

  type PageData = ResolvedPageData<RouteData>;

  let { data }: { data: PageData } = $props();

  function prettyDate(value: string): string {
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
  }

  function textField(
    event: MergeRequestThreadEvent,
    key: "subject_ref" | "decision" | "reason" | "body",
  ): string | null {
    let value: unknown;
    if (key === "subject_ref" && "subject_ref" in event) value = event.subject_ref;
    if (key === "decision" && "decision" in event) value = event.decision;
    if (key === "reason" && "reason" in event) value = event.reason;
    if (key === "body" && "body" in event) value = event.body;
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
      <div class="ticket-detail-main">
        <section class="ticket-detail-section">
          <div class="ticket-section-heading"><h2>Overview</h2></div>
          <dl class="ticket-facts">
            <div><dt>Repository</dt><dd>{mergeRequest.repository_key}</dd></div>
            <div><dt>Phase</dt><dd>{mergeRequest.state.kind}</dd></div>
            <div><dt>Current MR assignment</dt><dd>
              {#if mergeRequest.worker_assignment?.worker && mergeRequest.worker_resource_key}
                <a href={`/w/${encodeURIComponent(data.workspaceId)}/workers/${encodeURIComponent(mergeRequest.worker_resource_key)}`}>{mergeRequest.worker_resource_key}</a>
              {:else}
                {mergeRequest.worker_assignment?.worker ? "Assigned — public reference unavailable" : "Unassigned"}
              {/if}
              {#if mergeRequest.worker_assignment?.worker}
                <details><summary>Worker identity</summary>
                  <dl>
                    <div><dt>Runtime</dt><dd><code>{mergeRequest.worker_assignment.worker.runtime_id}</code></dd></div>
                    <div><dt>Worker</dt><dd><code>{mergeRequest.worker_assignment.worker.worker_id}</code></dd></div>
                  </dl>
                </details>
              {/if}
            </dd></div>
            <div><dt>Source selector</dt><dd><code>{mergeRequest.selector_from ?? "requires repair"}</code></dd></div>
            <div><dt>{mergeRequest.state.kind === "completed" ? "Recorded source ref" : "Live source ref"}</dt><dd>{mergeRequest.source.status}{mergeRequest.source.ref ? ` · ${mergeRequest.source.ref}` : ""}</dd></div>
            {#if mergeRequest.source.diagnostic}
              <div><dt>Source observation error</dt><dd>{mergeRequest.source.diagnostic.code}: {mergeRequest.source.diagnostic.message}</dd></div>
            {/if}
            <div><dt>Target selector</dt><dd><code>{mergeRequest.selector_to}</code></dd></div>
            <div><dt>Target ref</dt><dd>{mergeRequest.target.status}{mergeRequest.target.ref ? ` · ${mergeRequest.target.ref}` : ""}</dd></div>
            {#if mergeRequest.target.diagnostic}
              <div><dt>Target observation error</dt><dd>{mergeRequest.target.diagnostic.code}: {mergeRequest.target.diagnostic.message}</dd></div>
            {/if}
            <div><dt>Updated</dt><dd>{prettyDate(mergeRequest.updated_at)}</dd></div>
          </dl>
        </section>

        <section class="ticket-detail-section">
          <div class="ticket-section-heading"><h2>Review and integration status</h2></div>
          <dl class="ticket-facts">
            <div><dt>{mergeRequest.state.kind === "completed" ? "Recorded integration approval" : "Live source review freshness"}</dt><dd>{sourceReviewFreshness(mergeRequest)}</dd></div>
            <div><dt>Target integration</dt><dd>{targetIntegrationStatus(mergeRequest)}</dd></div>
            {#if mergeRequest.state.kind === "implementation" || mergeRequest.state.kind === "integration"}
              <div><dt>Backend readiness evaluation</dt><dd>{mergeRequest.summary.readiness.ready ? "Ready" : "Not ready"}
                {#if mergeRequest.summary.readiness.blockers.length}
                  <ul>{#each mergeRequest.summary.readiness.blockers as blocker}<li>{blocker}</li>{/each}</ul>
                {/if}
              </dd></div>
            {/if}
          </dl>
        </section>

        <section class="ticket-detail-section">
          <div class="ticket-section-heading">
            <h2>Thread</h2><span>{mergeRequest.thread.length} of {mergeRequest.summary.thread_event_count}</span>
          </div>
          <div class="ticket-timeline">
            {#each mergeRequest.thread as event (event.sequence)}
              <article>
                <div class="ticket-timeline-marker"></div>
                <div>
                  <header>
                    <strong>{event.kind}</strong>
                    <time>{prettyDate(event.created_at)}</time>
                  </header>
                  {#if textField(event, "subject_ref")}
                    <p><code>{textField(event, "subject_ref")}</code></p>
                  {/if}
                  {#if textField(event, "decision")}<p>{textField(event, "decision")}</p>{/if}
                  {#if textField(event, "reason")}<p>{textField(event, "reason")}</p>{/if}
                  {#if textField(event, "body")}<p class="mr-event-body">{textField(event, "body")}</p>{/if}
                  {#if event.kind === "review" && event.findings.length > 0}
                    <ul class="mr-findings" aria-label="Review findings">
                      {#each event.findings as finding}
                        <li>
                          <div class="mr-finding-location">
                            <strong>{finding.severity}</strong>
                            {#if finding.code}<code>{finding.code}</code>{/if}
                            {#if finding.path}<code>{finding.path}</code>{/if}
                            {#if finding.line != null}<span>Line {finding.line}</span>{/if}
                          </div>
                          <p class="mr-finding-body">{finding.body}</p>
                        </li>
                      {/each}
                    </ul>
                  {/if}
                </div>
              </article>
            {:else}
              <p class="workspace-empty-copy">No Merge Request events.</p>
            {/each}
          </div>
          <nav class="mr-pages" aria-label="Thread pages">
            {#if data.after !== null}
              <a class="workspace-secondary-button" href={mergeRequestPagePath(data.workspaceId, mergeRequest.merge_request_id)}>First events</a>
            {/if}
            {#if mergeRequest.event_page.has_more && mergeRequest.event_page.next_after != null}
              <a class="workspace-secondary-button" href={`${mergeRequestPagePath(data.workspaceId, mergeRequest.merge_request_id)}?after=${mergeRequest.event_page.next_after}`}>Next events</a>
            {/if}
          </nav>
        </section>
      </div>

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

<style>
  .workspace-page-header { margin-bottom: var(--space-5); }
  .ticket-detail-section, .ticket-control-card { border: 0; border-radius: 0; background: transparent; padding: 0; }
  .ticket-detail-main { gap: var(--space-6); }
  .ticket-detail-main, .ticket-facts dd { min-width: 0; overflow-wrap: anywhere; }
  .workspace-page-lede { overflow-wrap: anywhere; }
  .mr-findings { list-style: none; padding: 0; margin-top: var(--space-3); }
  .mr-findings li + li { margin-top: var(--space-4); }
  .mr-finding-location { display: flex; flex-wrap: wrap; gap: var(--space-2); overflow-wrap: anywhere; }
  .mr-finding-location code { min-width: 0; }
  .mr-event-body, .mr-finding-body { white-space: pre-wrap; overflow-wrap: anywhere; }
  .ticket-timeline article > div { min-width: 0; }
  .mr-pages { display: flex; gap: var(--space-2); flex-wrap: wrap; margin-top: var(--space-3); }
</style>
