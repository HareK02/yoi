<script lang="ts">
  import { untrack } from 'svelte';
  import DocumentMarkdown from '#lib/workspace/markdown/DocumentMarkdown.svelte';
  import { formatDate, workspaceRoute } from '#lib/workspace/api/http.ts';
  import {
    SubjektivSubjectCreateError,
    updateSubjektivSubjectBehavior,
  } from '#lib/workspace/memory/api.ts';
  import type { SubjektivMemoryState } from '#lib/generated/memory-api.ts';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();

  let subject = $state(untrack(() => data.subject.data));
  let editingBehavior = $state(false);
  let behaviorDraft = $state(untrack(() => subject?.behavior_md ?? ''));
  let behaviorSaving = $state(false);
  let behaviorError = $state<string | null>(null);
  let behaviorSaveConfirmed = $state(false);

  async function saveBehavior(): Promise<void> {
    if (!subject || behaviorSaving) return;
    behaviorSaving = true;
    behaviorError = null;
    behaviorSaveConfirmed = false;
    try {
      subject = await updateSubjektivSubjectBehavior(fetch, data.workspaceId, data.subjectId, {
        expected_behavior_revision: subject.behavior_revision,
        behavior_md: behaviorDraft,
      });
      behaviorDraft = subject.behavior_md;
      editingBehavior = false;
      behaviorSaveConfirmed = true;
    } catch (error) {
      behaviorError = error instanceof SubjektivSubjectCreateError
        ? (error.status === 401 || error.status === 403
          ? 'You do not have permission to edit this Subject.'
          : error.message)
        : 'The save outcome is unknown. Reload the Subject before retrying.';
    } finally {
      behaviorSaving = false;
    }
  }

  function beginBehaviorEdit(): void {
    if (!subject) return;
    behaviorDraft = subject.behavior_md;
    behaviorError = null;
    behaviorSaveConfirmed = false;
    editingBehavior = true;
  }

  function cancelBehaviorEdit(): void {
    behaviorDraft = subject?.behavior_md ?? '';
    behaviorError = null;
    editingBehavior = false;
  }

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
</script>

<svelte:head>
  <title>{subject?.role ?? data.subjectId} · Memory · Yoi Workspace</title>
  <meta name="description" content="Subject resident surface and current committed Memories" />
</svelte:head>

<section class="memory-page memory-subject-page" aria-labelledby="subject-heading" data-memory-view="subject" data-memory-state={surface?.availability ?? (data.surface.error ? 'error' : 'unavailable')}>
  <a class="memory-back-link" href={subjectsHref()}>← All subjects</a>

  <header class="memory-page-header">
    <div>
      <p class="memory-eyebrow">Memory subject</p>
      <h1 id="subject-heading">{subject?.role ?? data.subjectId}</h1>
      <code class="subject-id" title={data.subjectId}>{data.subjectId}</code>
    </div>
    <span class="read-only-label">Behavior editable</span>
  </header>

  {#if subject}
    <dl class="subject-facts" aria-label="Subject details">
      <div><dt>Role</dt><dd>{subject.role}</dd></div>
      <div><dt>State</dt><dd><span class="subject-state is-{subject.state}">{subject.state}</span></dd></div>
      <div><dt>Store revision</dt><dd>{subject.store_revision}</dd></div>
      <div><dt>Updated</dt><dd><time datetime={subject.updated_at}>{formatDate(subject.updated_at)}</time></dd></div>
      <div><dt>Current worker</dt><dd>{subject.current_worker?.display_name ?? 'None'}</dd></div>
    </dl>
  {:else if data.subject.error}
    <div class="memory-state is-error" role="alert">
      <strong>Subject details unavailable.</strong>
      <p>{data.subject.error}</p>
    </div>
  {:else}
    <div class="memory-state" role="status"><p>Subject details are unavailable.</p></div>
  {/if}

  {#if subject}
    <section class="behavior-section" aria-labelledby="subject-behavior-heading">
      <header class="section-heading">
        <div>
          <p class="memory-eyebrow">User-managed context</p>
          <h2 id="subject-behavior-heading">Behavior</h2>
        </div>
        {#if !editingBehavior}
          <button type="button" class="behavior-edit" onclick={beginBehaviorEdit}>Edit</button>
        {/if}
      </header>
      <p class="behavior-help">The Host injects this text verbatim for connected Workers, separately from generated Memory. Saving confirms storage only. Connected Workers check for the latest revision before later model requests; this page does not report that application.</p>
      {#if editingBehavior}
        <form class="behavior-form" aria-busy={behaviorSaving} onsubmit={(event) => { event.preventDefault(); void saveBehavior(); }}>
          <label>
            <span class="sr-only">Subject behavior</span>
            <textarea bind:value={behaviorDraft} rows="9" disabled={behaviorSaving} aria-invalid={behaviorError ? 'true' : undefined} aria-describedby={behaviorError ? 'subject-behavior-edit-error' : undefined}></textarea>
          </label>
          {#if behaviorError}<p id="subject-behavior-edit-error" class="behavior-error" role="alert">{behaviorError}</p>{/if}
          <div class="behavior-actions">
            <button type="submit" disabled={behaviorSaving}>{behaviorSaving ? 'Saving…' : 'Save behavior'}</button>
            <button type="button" disabled={behaviorSaving} onclick={cancelBehaviorEdit}>Cancel</button>
            <button type="button" disabled={behaviorSaving || behaviorDraft.length === 0} onclick={() => { behaviorDraft = ''; }}>Clear</button>
          </div>
        </form>
      {:else if subject.behavior_md.length > 0}
        <article class="behavior-document" aria-label="User-managed Subject behavior"><pre>{subject.behavior_md}</pre></article>
      {:else}
        <div class="memory-state" role="status"><strong>No behavior is set.</strong><p>Connected Workers receive an explicit empty behavior document.</p></div>
      {/if}
      {#if behaviorSaveConfirmed}<p class="behavior-saved" role="status">Behavior storage confirmed at revision {subject.behavior_revision}. This page does not confirm application by a connected Worker.</p>{/if}
    </section>
  {/if}

  <section class="surface-section" aria-labelledby="resident-surface-heading">
    <header class="section-heading">
      <div>
        <p class="memory-eyebrow">Resident context</p>
        <h2 id="resident-surface-heading">Resident surface</h2>
      </div>
      {#if surface}
        <span class="availability is-{surface.availability}">{surface.availability}</span>
      {/if}
    </header>

    {#if surface?.availability === 'ready' && snapshot}
      <div class="surface-meta">
        <span>Built from store revision {snapshot.built_from_store_revision}</span>
        <span aria-hidden="true">·</span>
        <time datetime={snapshot.created_at}>{formatDate(snapshot.created_at)}</time>
        <span aria-hidden="true">·</span>
        <span>{snapshot.memory_refs.length} Memory ref{snapshot.memory_refs.length === 1 ? '' : 's'}</span>
      </div>
      {#if snapshot.body_md.trim().length === 0}
        <div class="memory-state" role="status" data-surface-ready-empty>
          <strong>Resident surface is ready and empty.</strong>
          <p>This subject currently has no resident context to display.</p>
        </div>
      {:else}
        <article class="surface-document" aria-label="Resident surface">
          <DocumentMarkdown text={snapshot.body_md} />
        </article>
      {/if}
      <details class="surface-details">
        <summary>Surface provenance</summary>
        <dl>
          <div><dt>Snapshot id</dt><dd><code>{snapshot.snapshot_id}</code></dd></div>
          <div>
            <dt>Memory refs</dt>
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
        <strong>Resident surface has not been generated.</strong>
        <p>Committed Memories remain available below.</p>
      </div>
    {:else if surface?.availability === 'stale'}
      <div class="memory-state is-warning" role="status">
        <strong>Resident surface is stale.</strong>
        <p>The latest committed Memories are not represented by a publishable snapshot.</p>
      </div>
    {:else if surface?.availability === 'failed'}
      <div class="memory-state is-error" role="alert">
        <strong>Resident surface generation failed.</strong>
        <p>No failed or partial snapshot is displayed.</p>
      </div>
    {:else if data.surface.error}
      <div class="memory-state is-error" role="alert">
        <strong>Resident surface unavailable.</strong>
        <p>{data.surface.error}</p>
      </div>
    {:else}
      <div class="memory-state" role="status"><p>Resident surface data is unavailable.</p></div>
    {/if}
  </section>

  <section class="memories-section" aria-labelledby="current-memories-heading">
    <header class="section-heading">
      <div>
        <p class="memory-eyebrow">Committed records</p>
        <h2 id="current-memories-heading">Current Memories</h2>
      </div>
      {#if data.memories.data}<span class="memory-count">{memories.length}{data.memories.data.has_more ? '+' : ''}</span>{/if}
    </header>

    {#if data.memories.data}
      {#if memories.length === 0}
        <div class="memory-state" role="status" data-memories-empty>
          <strong>No committed Memories.</strong>
          <p>This subject has no current Memory revisions.</p>
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
                <div><dt>Memory id</dt><dd><code title={memory.id}>{memory.id}</code></dd></div>
                <div><dt>Revision</dt><dd>{memory.revision}</dd></div>
                <div><dt>Updated</dt><dd><time datetime={memory.updated_at}>{formatDate(memory.updated_at)}</time></dd></div>
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

  .subject-facts {
    grid-template-columns: repeat(5, minmax(0, 1fr));
    gap: var(--space-3);
  }

  .subject-facts > div {
    display: block;
  }

  .subject-facts dt {
    white-space: normal;
  }

  .subject-facts dd {
    margin-top: var(--space-1);
    color: var(--text);
    overflow-wrap: anywhere;
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

  .behavior-section,
  .surface-section,
  .memories-section {
    display: grid;
    gap: var(--space-4);
    min-width: 0;
    padding-top: var(--space-3);
  }

  .behavior-help,
  .behavior-saved,
  .behavior-error {
    margin: 0;
    color: var(--text-muted);
    font-size: var(--font-size-compact);
  }

  .behavior-error { color: var(--danger); }
  .behavior-saved { color: var(--success); }

  .behavior-form,
  .behavior-form label {
    display: grid;
    gap: var(--space-3);
  }

  .behavior-form textarea {
    width: 100%;
    min-height: 10rem;
    padding: var(--space-3);
    color: var(--text);
    background: var(--surface-raised);
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-sm);
    font: inherit;
    line-height: 1.5;
    resize: vertical;
  }

  .behavior-actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }

  .behavior-edit,
  .behavior-actions button {
    padding: var(--space-2) var(--space-3);
    color: var(--text);
    background: var(--surface-raised);
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }

  .behavior-actions button:disabled { cursor: not-allowed; opacity: 0.55; }

  .behavior-document pre {
    margin: 0;
    padding: var(--space-4);
    overflow-wrap: anywhere;
    white-space: pre-wrap;
    background: var(--surface-raised);
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    font: inherit;
    line-height: 1.6;
  }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
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
    .subject-facts {
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

    .subject-facts {
      grid-template-columns: minmax(0, 1fr);
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
