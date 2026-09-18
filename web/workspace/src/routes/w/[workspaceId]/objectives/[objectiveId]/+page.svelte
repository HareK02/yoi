<script lang="ts">
  import RichMarkdown from '$lib/workspace/console/RichMarkdown.svelte';
  import { formatDate } from '$lib/workspace/api/http';
  import { ticketHref } from '$lib/workspace/resource-links';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();

  const objectivesHref = (workspaceId: string) =>
    `/w/${encodeURIComponent(workspaceId)}/objectives`;
</script>

<svelte:head>
  <title>{data.objective?.title ?? data.objectiveId} · Objective</title>
</svelte:head>

<section class="card objective-detail-card">
  <a class="objective-back-link" href={objectivesHref(data.workspaceId)}>← All Objectives</a>
  <h2>Objective detail</h2>
  {#if data.objective}
    <div class="objective-title-row detail">
      <h3>{data.objective.title}</h3>
      <span class="state-pill">{data.objective.state}</span>
    </div>
    <dl class="objective-detail-metadata">
      <div>
        <dt>Created</dt>
        <dd>{data.objective.created_at ? formatDate(data.objective.created_at) : 'unknown'}</dd>
      </div>
      <div>
        <dt>Updated</dt>
        <dd>{data.objective.updated_at ? formatDate(data.objective.updated_at) : 'unknown'}</dd>
      </div>
      <div>
        <dt>Record source</dt>
        <dd>{data.objective.record_source}</dd>
      </div>
      <div>
        <dt>Linked tickets</dt>
        <dd>
          {#if data.objective.linked_ticket_summaries.length}
            {#each data.objective.linked_ticket_summaries as ticket, index}
              {#if index}, {/if}<a href={ticketHref(data.workspaceId, ticket)}>{ticket.resource_key}</a>
            {/each}
          {:else}
            none
          {/if}
        </dd>
      </div>
    </dl>
    <div class="objective-body" aria-label="Objective body">
      {#if data.objective.body}
        <RichMarkdown text={data.objective.body} />
      {:else}
        <p class="section-note">No Objective body is available.</p>
      {/if}
    </div>
    {#if data.objective.body_truncated}
      <p class="error">Objective body was truncated by the Backend response limit.</p>
    {/if}
  {:else if data.objectiveError}
    <p class="error">{data.objectiveError}</p>
  {:else}
    <p>Loading objective detail…</p>
  {/if}
</section>
