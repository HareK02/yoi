<script lang="ts">
  import { formatDate, workspaceRoute } from '$lib/workspace/api/http';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();

  const subjects = $derived(data.subjects.data?.items ?? []);

  function subjectHref(subjectId: string): string {
    return workspaceRoute(data.workspaceId, `/memory/${encodeURIComponent(subjectId)}`);
  }
</script>

<svelte:head>
  <title>Subjects · Memory · Yoi Workspace</title>
  <meta name="description" content="Workspace Memory subjects" />
</svelte:head>

<section class="memory-page memory-subject-index" aria-labelledby="memory-subjects-heading" data-memory-view="subjects">
  <header class="memory-page-header">
    <div>
      <p class="memory-eyebrow">Memory</p>
      <h1 id="memory-subjects-heading">Subjects</h1>
      <p>Read-only durable context, organized by explicit subject.</p>
    </div>
    {#if data.subjects.data}
      <span class="memory-count">{subjects.length}{data.subjects.data.has_more ? '+' : ''} subject{subjects.length === 1 ? '' : 's'}</span>
    {/if}
  </header>

  {#if data.subjects.data}
    {#if subjects.length === 0}
      <div class="memory-state" role="status" data-memory-state="empty">
        <strong>No Memory subjects.</strong>
        <p>Subjects will appear here after they are created for this Workspace.</p>
      </div>
    {:else}
      <div class="subject-list" aria-label="Memory subjects">
        {#each subjects as subject (subject.id)}
          <a class="subject-row" href={subjectHref(subject.id)}>
            <div class="subject-copy">
              <span class="memory-state-pill is-{subject.state}">{subject.state}</span>
              <h2>{subject.role}</h2>
              <code title={subject.id}>{subject.id}</code>
            </div>
            <dl class="subject-meta">
              <div><dt>Store revision</dt><dd>{subject.store_revision}</dd></div>
              <div><dt>Updated</dt><dd><time datetime={subject.updated_at}>{formatDate(subject.updated_at)}</time></dd></div>
              <div><dt>Worker</dt><dd>{subject.current_worker?.display_name ?? 'None'}</dd></div>
            </dl>
          </a>
        {/each}
      </div>
      {#if data.subjects.data.has_more}
        <p class="memory-note">Only the first {data.subjects.data.limit} subjects are shown.</p>
      {/if}
    {/if}
  {:else if data.subjects.error}
    <div class="memory-state is-error" role="alert" data-memory-state="error">
      <strong>Subjects unavailable.</strong>
      <p>{data.subjects.error}</p>
    </div>
  {:else}
    <div class="memory-state" role="status" data-memory-state="unavailable">
      <p>Subject data is unavailable.</p>
    </div>
  {/if}
</section>

<style>
  .memory-page {
    display: grid;
    flex: 0 0 auto;
    gap: var(--space-5);
    width: 100%;
    min-width: 0;
    max-width: 78rem;
    margin-inline: auto;
  }

  .memory-page-header {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    gap: var(--space-5);
    padding-bottom: var(--space-4);
    border-bottom: 1px solid var(--line);
  }

  .memory-page-header h1,
  .subject-copy h2 {
    margin: 0;
    color: var(--text-strong);
  }

  .memory-page-header h1 {
    font-size: var(--font-size-title);
  }

  .memory-page-header p:not(.memory-eyebrow) {
    max-width: 46rem;
    margin: var(--space-2) 0 0;
    color: var(--text-muted);
  }

  .memory-eyebrow {
    margin: 0 0 var(--space-1);
    color: var(--accent);
    font-size: var(--font-size-compact);
    font-weight: 800;
    letter-spacing: 0.1em;
    text-transform: uppercase;
  }

  .memory-count {
    flex: 0 0 auto;
    color: var(--text-muted);
    font-size: var(--font-size-compact);
  }

  .subject-list {
    display: grid;
    border-top: 1px solid var(--line);
  }

  .subject-row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(22rem, 0.8fr);
    gap: var(--space-5);
    min-width: 0;
    padding: var(--space-4);
    border-bottom: 1px solid var(--line);
    color: inherit;
    text-decoration: none;
  }

  .subject-row:hover,
  .subject-row:focus-visible {
    background: var(--interactive-hover);
  }

  .subject-copy {
    display: grid;
    justify-items: start;
    gap: var(--space-1);
    min-width: 0;
  }

  .subject-copy h2 {
    font-size: var(--font-size-body);
    overflow-wrap: anywhere;
  }

  .subject-copy code {
    display: block;
    max-width: 100%;
    color: var(--text-muted);
    font-size: var(--font-size-compact);
    overflow-wrap: anywhere;
  }

  .memory-state-pill {
    color: var(--success);
    font-size: var(--font-size-compact);
    font-weight: 800;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .memory-state-pill.is-retired {
    color: var(--text-faint);
  }

  .subject-meta {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: var(--space-3);
    align-self: center;
  }

  .subject-meta div {
    display: block;
    min-width: 0;
  }

  .subject-meta dt {
    white-space: normal;
  }

  .subject-meta dd {
    margin-top: var(--space-1);
    color: var(--text-muted);
    font-size: var(--font-size-compact);
    overflow-wrap: anywhere;
  }

  .memory-state {
    padding: var(--space-5) 0;
    color: var(--text-muted);
  }

  .memory-state strong {
    color: var(--text-strong);
  }

  .memory-state p,
  .memory-note {
    margin: var(--space-1) 0 0;
    color: var(--text-muted);
  }

  .memory-state.is-error,
  .memory-state.is-error strong {
    color: var(--danger);
  }

  @media (max-width: 900px) {
    .subject-row {
      grid-template-columns: minmax(0, 1fr);
      gap: var(--space-3);
    }
  }

  @media (max-width: 600px) {
    .memory-page-header {
      display: grid;
      gap: var(--space-2);
    }

    .subject-row {
      padding-inline: 0;
    }

    .subject-meta {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
