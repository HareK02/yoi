<script lang="ts">
  import { formatDate, workspaceRoute } from '$lib/workspace-api/http';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
</script>

<svelte:head>
  <title>{data.objective?.title ?? data.objectiveId} · Objective</title>
</svelte:head>

<section class="card">
  <h2>Objectives</h2>
  {#if data.objectives}
    <div class="objective-list compact">
      {#each data.objectives.items as objective (objective.id)}
        <a class="objective-row" class:active={objective.id === data.objectiveId} href={workspaceRoute(data.workspaceId, `/objectives/${objective.id}`)}>
          <div class="objective-main">
            <div class="objective-title-row">
              <strong class="objective-title">{objective.title}</strong>
              <span class="state-pill">{objective.state}</span>
            </div>
            <p class="objective-summary">{objective.summary || 'No summary text is available.'}</p>
          </div>
          <div class="objective-meta" aria-label="Objective metadata">
            <span>Updated {objective.updated_at ? formatDate(objective.updated_at) : 'unknown'}</span>
            <code>{objective.id}</code>
          </div>
        </a>
      {/each}
    </div>
  {:else if data.objectivesError}
    <p class="error">{data.objectivesError}</p>
  {:else}
    <p>Loading objectives…</p>
  {/if}
</section>

<section class="card objective-detail-card">
  <h2>Objective detail</h2>
  {#if data.objective}
    <div class="objective-title-row detail">
      <div>
        <h3>{data.objective.title}</h3>
        <p><code>{data.objective.id}</code></p>
      </div>
      <span class="state-pill">{data.objective.state}</span>
    </div>
    <dl>
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
        <dd>{data.objective.linked_tickets.length ? data.objective.linked_tickets.join(', ') : 'none'}</dd>
      </div>
    </dl>
    <pre class="objective-body">{data.objective.body}</pre>
    {#if data.objective.body_truncated}
      <p class="error">Objective body was truncated by the Backend response limit.</p>
    {/if}
  {:else if data.objectiveError}
    <p class="error">{data.objectiveError}</p>
  {:else}
    <p>Loading objective detail…</p>
  {/if}
</section>
