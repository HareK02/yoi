<script lang="ts">
  import DocumentMarkdown from '#lib/workspace/markdown/DocumentMarkdown.svelte';
  import { formatDate, workspaceRoute } from '#lib/workspace/api/http.ts';
  import type {
    SubjektivMemoryState,
    SubjektivResidentSurfaceAvailability,
  } from '#lib/generated/memory-api.ts';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();

  const surface = $derived(data.surface.data);
  const snapshot = $derived(surface?.snapshot ?? null);
  const memories = $derived(data.memories.data?.items ?? []);

  function subjectsHref(): string {
    return workspaceRoute(data.workspaceId, '/memory');
  }

  function memoryHref(memoryId: string): string {
    return workspaceRoute(
      data.workspaceId,
      `/memory/${encodeURIComponent(data.subjectId)}/${encodeURIComponent(memoryId)}`,
    );
  }

  function memoryPageHref(cursor?: string | null): string {
    const path = workspaceRoute(
      data.workspaceId,
      `/memory/${encodeURIComponent(data.subjectId)}`,
    );
    return cursor ? `${path}?cursor=${encodeURIComponent(cursor)}` : path;
  }

  function stateLabel(state: SubjektivMemoryState): string {
    return state[0].toUpperCase() + state.slice(1);
  }

  function kindLabel(kind: string): string {
    return kind.replaceAll('_', ' ');
  }

  function surfaceLabel(availability: SubjektivResidentSurfaceAvailability): string {
    if (availability === 'ready') return snapshot?.body_md.trim() ? 'Ready to use' : 'Ready, no context';
    if (availability === 'ungenerated') return 'Not generated';
    if (availability === 'stale') return 'Needs refresh';
    return 'Generation failed';
  }

  function surfaceSummary(availability: SubjektivResidentSurfaceAvailability): string {
    if (availability !== 'ready') return 'No current generated context is available to use.';
    return snapshot?.body_md.trim()
      ? 'Current generated context is available.'
      : 'A current empty surface exists; there is no resident context to show.';
  }

  function subjectStateLabel(state: 'active' | 'retired'): string {
    return state === 'active' ? 'Available' : 'Retired';
  }
</script>

<svelte:head>
  <title>{data.subject.data?.role ?? data.subjectId} · Memory · Yoi Workspace</title>
  <meta name="description" content="Subject resident surface and current committed Memories" />
</svelte:head>

<section class="memory-page memory-subject-page" aria-labelledby="subject-heading" data-memory-view="subject" data-memory-state={surface?.availability ?? (data.surface.error ? 'error' : 'unavailable')}>
  <a class="memory-back-link" href={subjectsHref()}>← All subjects</a>

  <header class="memory-page-header">
    <div>
      <p class="memory-eyebrow">Memory subject</p>
      <h1 id="subject-heading">{data.subject.data?.role ?? data.subjectId}</h1>
      <code class="subject-id" title={data.subjectId}>{data.subjectId}</code>
    </div>
    <span class="read-only-label">Read-only</span>
  </header>

  {#if data.subject.data}
    <section class="subject-overview" aria-label="Subject status">
      <div>
        <span>Subject status</span>
        <strong class="subject-state is-{data.subject.data.state}">{subjectStateLabel(data.subject.data.state)}</strong>
        <p>{data.subject.data.state === 'active' ? 'Available for new Worker connections.' : 'No longer available for new Worker connections.'}</p>
      </div>
      <div>
        <span>Worker connection</span>
        <strong>{data.subject.data.current_worker ? 'Connected' : 'Not connected'}</strong>
        <p>{data.subject.data.current_worker?.display_name ?? 'No Worker currently owns this Subject connection.'}</p>
      </div>
      <div>
        <span>Committed Memories</span>
        {#if data.memories.data}
          <strong>{data.cursor ? `${memories.length} on this page` : memories.length === 0 ? 'None' : 'Available'}</strong>
          <p>{memories.length} shown{data.memories.data.has_more ? ' · more available' : ''}</p>
        {:else}
          <strong>Unavailable</strong>
          <p>The current Memory list could not be read.</p>
        {/if}
      </div>
      <div>
        <span>Resident context</span>
        {#if surface}
          <strong class="availability is-{surface.availability}">{surfaceLabel(surface.availability)}</strong>
          <p>{surfaceSummary(surface.availability)}</p>
        {:else}
          <strong>Unavailable</strong>
          <p>The resident context status could not be read.</p>
        {/if}
      </div>
    </section>

    <details class="subject-technical-details">
      <summary>Technical details</summary>
      <dl>
        <div><dt>Subject ID</dt><dd><code>{data.subject.data.id}</code></dd></div>
        <div>
          <dt>Subject store revision</dt>
          <dd>{data.subject.data.store_revision}<small>Internal change number for committed Memories; not a Memory count or content-quality score.</small></dd>
        </div>
        <div><dt>Last changed</dt><dd><time datetime={data.subject.data.updated_at}>{formatDate(data.subject.data.updated_at)}</time></dd></div>
      </dl>
    </details>
  {:else if data.subject.error}
    <div class="memory-state is-error" role="alert">
      <strong>Subject details unavailable.</strong>
      <p>{data.subject.error}</p>
    </div>
  {:else}
    <div class="memory-state" role="status"><p>Subject details are unavailable.</p></div>
  {/if}

  <section class="surface-section" aria-labelledby="resident-surface-heading">
    <header class="section-heading">
      <div>
        <p class="memory-eyebrow">Generated from committed Memory</p>
        <h2 id="resident-surface-heading">Resident context</h2>
      </div>
      {#if surface}
        <span class="availability is-{surface.availability}">{surfaceLabel(surface.availability)}</span>
      {/if}
    </header>

    {#if surface?.availability === 'ready' && snapshot}
      <div class="surface-meta">
        <span>Generated <time datetime={snapshot.created_at}>{formatDate(snapshot.created_at)}</time></span>
        <span aria-hidden="true">·</span>
        <span>{snapshot.memory_refs.length} referenced Memory {snapshot.memory_refs.length === 1 ? 'record' : 'records'}</span>
      </div>
      {#if snapshot.body_md.trim().length === 0}
        <div class="memory-state" role="status" data-surface-ready-empty>
          <strong>Resident context is current but empty.</strong>
          <p>A generated surface exists, but it contains no context to show. Committed Memories, if any, remain listed below.</p>
        </div>
      {:else}
        <article class="surface-document" aria-label="Resident context">
          <DocumentMarkdown text={snapshot.body_md} />
        </article>
      {/if}
      <details class="surface-details">
        <summary>Surface sources and diagnostics</summary>
        <dl>
          <div><dt>Snapshot ID</dt><dd><code>{snapshot.snapshot_id}</code></dd></div>
          <div>
            <dt>Generated from Subject revision</dt>
            <dd>{snapshot.built_from_store_revision}<small>The Subject change number used for this surface, distinct from each Memory’s revision.</small></dd>
          </div>
          <div>
            <dt>Referenced Memory revisions</dt>
            <dd>
              {#if snapshot.memory_refs.length === 0}
                None
              {:else}
                <ul class="reference-list">
                  {#each snapshot.memory_refs as reference (`${reference.memory_id}:${reference.revision}`)}
                    <li><a href={memoryHref(reference.memory_id)}><code>{reference.memory_id}</code> · revision {reference.revision}</a></li>
                  {/each}
                </ul>
              {/if}
            </dd>
          </div>
        </dl>
      </details>
    {:else if surface?.availability === 'ungenerated'}
      <div class="memory-state" role="status">
        <strong>Resident context has not been generated.</strong>
        <p>Committed Memories, if any, remain available below.</p>
      </div>
    {:else if surface?.availability === 'stale'}
      <div class="memory-state is-warning" role="status">
        <strong>Resident context needs to be refreshed.</strong>
        <p>The latest committed Memories are not represented by a current surface, so no generated context is shown.</p>
      </div>
    {:else if surface?.availability === 'failed'}
      <div class="memory-state is-error" role="alert">
        <strong>Resident context generation failed.</strong>
        <p>No failed or partial surface is shown. Committed Memories remain available below.</p>
      </div>
    {:else if data.surface.error}
      <div class="memory-state is-error" role="alert">
        <strong>Resident context status unavailable.</strong>
        <p>{data.surface.error}</p>
      </div>
    {:else}
      <div class="memory-state" role="status"><p>Resident context status is unavailable.</p></div>
    {/if}
  </section>

  <section class="memories-section" aria-labelledby="current-memories-heading">
    <header class="section-heading">
      <div>
        <p class="memory-eyebrow">Committed records</p>
        <h2 id="current-memories-heading">Current Memories</h2>
      </div>
      {#if data.memories.data}<span class="memory-count">{memories.length} shown{data.memories.data.has_more ? ' · more available' : ''}</span>{/if}
    </header>

    {#if data.memories.data}
      {#if memories.length === 0}
        <div class="memory-state" role="status" data-memories-empty>
          <strong>{data.cursor ? 'No committed Memories on this page.' : 'No committed Memories yet.'}</strong>
          <p>{data.cursor ? 'Return to the first page to review earlier records.' : 'This Subject has no current Memory records.'}</p>
          {#if data.cursor}<p><a href={memoryPageHref()}>Return to the first page</a></p>{/if}
        </div>
      {:else}
        <div class="memory-list" aria-label="Current committed Memories">
          {#each memories as memory (memory.id)}
            <a class="memory-row" href={memoryHref(memory.id)}>
              <div class="memory-row-heading">
                <div class="pill-row">
                  <span class="memory-pill is-{memory.state}">{stateLabel(memory.state)}</span>
                  <span class="memory-kind">{kindLabel(memory.kind)}</span>
                </div>
                <h3>{memory.claim}</h3>
                <p>{memory.excerpt || 'No excerpt.'}</p>
              </div>
              <dl>
                <div><dt>Memory ID</dt><dd><code title={memory.id}>{memory.id}</code></dd></div>
                <div><dt>Memory revision</dt><dd>{memory.revision}</dd></div>
                <div><dt>Last changed</dt><dd><time datetime={memory.updated_at}>{formatDate(memory.updated_at)}</time></dd></div>
              </dl>
            </a>
          {/each}
        </div>
        <nav class="memory-pagination" aria-label="Current Memory pages">
          {#if data.cursor}<a href={memoryPageHref()}>First page</a>{/if}
          {#if data.memories.data.has_more && data.memories.data.next_cursor}
            <a href={memoryPageHref(data.memories.data.next_cursor)}>Next page →</a>
          {/if}
        </nav>
      {/if}
    {:else if data.memories.error}
      <div class="memory-state is-error" role="alert">
        <strong>Current Memories unavailable.</strong>
        <p>{data.memories.error}</p>
      </div>
    {:else}
      <div class="memory-state" role="status"><p>Current Memory data is unavailable.</p></div>
    {/if}
  </section>
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

  .memory-back-link {
    width: fit-content;
    color: var(--text-muted);
    font-size: var(--font-size-compact);
  }

  .memory-page-header,
  .section-heading {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    gap: var(--space-4);
  }

  .memory-page-header {
    padding-bottom: var(--space-4);
    border-bottom: 1px solid var(--line);
  }

  .memory-page-header h1,
  .section-heading h2,
  .memory-row h3 {
    margin: 0;
    color: var(--text-strong);
    overflow-wrap: anywhere;
  }

  .memory-page-header h1 {
    font-size: var(--font-size-title);
  }

  .section-heading h2 {
    font-size: var(--font-size-body);
  }

  .memory-eyebrow {
    margin: 0 0 var(--space-1);
    color: var(--accent);
    font-size: var(--font-size-compact);
    font-weight: 800;
    letter-spacing: 0.1em;
    text-transform: uppercase;
  }

  .subject-id {
    display: block;
    max-width: min(60rem, 100%);
    margin-top: var(--space-2);
    color: var(--text-muted);
    overflow-wrap: anywhere;
  }

  .read-only-label,
  .memory-count {
    flex: 0 0 auto;
    color: var(--text-muted);
    font-size: var(--font-size-compact);
  }

  .subject-overview {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: var(--space-4);
    padding: var(--space-4);
    background: var(--bg-raised);
    border-radius: var(--radius-soft);
  }

  .subject-overview > div {
    display: grid;
    align-content: start;
    gap: var(--space-1);
    min-width: 0;
  }

  .subject-overview span,
  .subject-overview p,
  .subject-technical-details summary,
  .subject-technical-details small,
  .surface-details small {
    color: var(--text-muted);
    font-size: var(--font-size-compact);
  }

  .subject-overview strong {
    color: var(--text-strong);
    overflow-wrap: anywhere;
  }

  .subject-overview p {
    margin: 0;
    overflow-wrap: anywhere;
  }

  .subject-technical-details {
    padding-bottom: var(--space-3);
    border-bottom: 1px solid var(--line);
  }

  .subject-technical-details summary {
    width: fit-content;
    cursor: pointer;
    font-weight: 700;
  }

  .subject-technical-details dl,
  .surface-details dl {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: var(--space-4);
    margin-top: var(--space-3);
  }

  .subject-technical-details dl > div,
  .surface-details dl > div {
    display: block;
    min-width: 0;
  }

  .subject-technical-details dd,
  .surface-details dd {
    margin-top: var(--space-1);
    overflow-wrap: anywhere;
  }

  .subject-technical-details small,
  .surface-details small {
    display: block;
    margin-top: var(--space-1);
    line-height: 1.4;
  }

  .subject-state,
  .availability,
  .memory-pill,
  .memory-kind {
    font-size: var(--font-size-compact);
    font-weight: 800;
    letter-spacing: 0.07em;
    text-transform: uppercase;
  }

  .subject-state,
  .availability.is-ready,
  .memory-pill.is-active {
    color: var(--success);
  }

  .subject-state.is-retired,
  .availability.is-ungenerated,
  .memory-pill.is-retracted {
    color: var(--text-faint);
  }

  .availability.is-stale,
  .memory-pill.is-resolved {
    color: var(--warning);
  }

  .availability.is-failed {
    color: var(--danger);
  }

  .surface-section,
  .memories-section {
    display: grid;
    gap: var(--space-4);
    min-width: 0;
    padding-top: var(--space-3);
  }

  .surface-meta,
  .pill-row {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    color: var(--text-muted);
    font-size: var(--font-size-compact);
  }

  .surface-document {
    min-width: 0;
  }

  .surface-details {
    padding-top: var(--space-3);
    border-top: 1px solid var(--line);
  }

  .surface-details summary {
    color: var(--text-muted);
    cursor: pointer;
    font-size: var(--font-size-compact);
    font-weight: 700;
  }

  .surface-details dl {
    margin-top: var(--space-3);
  }

  .surface-details dd,
  .reference-list code {
    overflow-wrap: anywhere;
  }

  .reference-list {
    display: grid;
    gap: var(--space-1);
    margin: 0;
    padding-left: 1.25rem;
  }

  .memory-list {
    display: grid;
    border-top: 1px solid var(--line);
  }

  .memory-row {
    display: grid;
    grid-template-columns: minmax(0, 1.4fr) minmax(19rem, 0.8fr);
    gap: var(--space-5);
    min-width: 0;
    padding: var(--space-4);
    border-bottom: 1px solid var(--line);
    color: inherit;
    text-decoration: none;
  }

  .memory-row:hover,
  .memory-row:focus-visible {
    background: var(--interactive-hover);
  }

  .memory-row-heading {
    display: grid;
    gap: var(--space-2);
    min-width: 0;
  }

  .memory-row h3 {
    font-size: var(--font-size-body);
  }

  .memory-row p {
    margin: 0;
    color: var(--text-muted);
  }

  .memory-kind {
    color: var(--accent);
  }

  .memory-row dl {
    align-content: center;
  }

  .memory-row dl > div {
    grid-template-columns: 6.5rem minmax(0, 1fr);
  }

  .memory-row dd,
  .memory-row code {
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .memory-state {
    padding: var(--space-4) 0;
    color: var(--text-muted);
  }

  .memory-pagination {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: var(--space-3);
    font-size: var(--font-size-compact);
  }

  .memory-state strong {
    color: var(--text-strong);
  }

  .memory-state p {
    margin: var(--space-1) 0 0;
  }

  .memory-state.is-warning,
  .memory-state.is-warning strong {
    color: var(--warning);
  }

  .memory-state.is-error,
  .memory-state.is-error strong {
    color: var(--danger);
  }

  @media (max-width: 900px) {
    .subject-overview {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }

    .memory-row {
      grid-template-columns: minmax(0, 1fr);
      gap: var(--space-3);
    }
  }

  @media (max-width: 600px) {
    .memory-page-header,
    .section-heading {
      display: grid;
      gap: var(--space-2);
    }

    .subject-overview,
    .subject-technical-details dl,
    .surface-details dl {
      grid-template-columns: minmax(0, 1fr);
    }

    .subject-overview {
      padding: var(--space-3);
    }

    .memory-row {
      padding-inline: 0;
    }

    .memory-row dl > div,
    .surface-details dl > div {
      grid-template-columns: minmax(0, 1fr);
      gap: var(--space-1);
    }
  }
</style>
