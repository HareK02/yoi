<script lang="ts">
  import { formatDate, workspaceRoute } from '$lib/workspace-api/http';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
</script>

<svelte:head>
  <title>Objectives · Yoi Workspace</title>
</svelte:head>

<section class="card">
  <h2>Objectives</h2>
  <p class="section-note">Objectives are read from canonical filesystem records through <code>/api/objectives</code>.</p>
  {#if data.objectives}
    {#if data.objectives.items.length === 0}
      <p>No Objective records are present.</p>
    {:else}
      <div class="objective-list">
        {#each data.objectives.items as objective (objective.id)}
          <a class="objective-row" href={workspaceRoute(data.workspaceId, `/objectives/${objective.id}`)}>
            <div class="objective-main">
              <div class="objective-title-row">
                <strong class="objective-title">{objective.title}</strong>
                <span class="state-pill">{objective.state}</span>
              </div>
              <p class="objective-summary">{objective.summary || 'No summary text is available.'}</p>
            </div>
            <div class="objective-meta" aria-label="Objective metadata">
              <span>Updated {objective.updated_at ? formatDate(objective.updated_at) : 'unknown'}</span>
              <span>{objective.linked_tickets?.length ? `${objective.linked_tickets.length} linked ticket(s)` : 'No linked tickets'}</span>
              <code>{objective.id}</code>
            </div>
          </a>
        {/each}
      </div>
    {/if}
    {#if data.objectives.invalid_records.length > 0}
      <p class="error">{data.objectives.invalid_records.length} invalid objective record(s) hidden.</p>
    {/if}
  {:else if data.objectivesError}
    <p class="error">{data.objectivesError}</p>
  {:else}
    <p>Waiting for <code>/api/objectives</code>…</p>
  {/if}
</section>
