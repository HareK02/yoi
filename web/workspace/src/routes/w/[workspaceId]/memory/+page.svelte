<script lang="ts">
  import { goto } from '$app/navigation';
  import { tick } from 'svelte';
  import { formatDate, workspaceRoute } from '#lib/workspace/api/http.ts';
  import {
    createSubjektivSubject,
    SubjektivSubjectCreateError,
  } from '#lib/workspace/memory/api.ts';
  import type { SubjektivSubjectResponse } from '#lib/generated/memory-api.ts';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();

  const subjects = $derived(data.subjects.data?.items ?? []);
  let createExpanded = $state(false);
  let role = $state('');
  let behaviorMd = $state('');
  let submitting = $state(false);
  let createError = $state<string | null>(null);
  let roleInvalid = $state(false);
  let behaviorInvalid = $state(false);
  let createOutcomeUnknown = $state(false);
  let createdSubject = $state<SubjektivSubjectResponse | null>(null);
  let createButton = $state<HTMLButtonElement>();
  let roleInput = $state<HTMLInputElement>();
  let behaviorInput = $state<HTMLTextAreaElement>();

  function subjectHref(subjectId: string): string {
    return workspaceRoute(data.workspaceId, `/memory/${encodeURIComponent(subjectId)}`);
  }

  function pageHref(cursor?: string | null): string {
    const path = workspaceRoute(data.workspaceId, '/memory');
    return cursor ? `${path}?cursor=${encodeURIComponent(cursor)}` : path;
  }

  async function openCreateForm(): Promise<void> {
    createExpanded = true;
    createError = null;
    roleInvalid = false;
    behaviorInvalid = false;
    createOutcomeUnknown = false;
    createdSubject = null;
    await tick();
    roleInput?.focus();
  }

  async function cancelCreate(): Promise<void> {
    if (submitting) return;
    createExpanded = false;
    role = '';
    behaviorMd = '';
    createError = null;
    roleInvalid = false;
    behaviorInvalid = false;
    createOutcomeUnknown = false;
    createdSubject = null;
    await tick();
    createButton?.focus();
  }

  async function submitCreate(): Promise<void> {
    if (submitting || createdSubject) return;
    submitting = true;
    createError = null;
    roleInvalid = false;
    behaviorInvalid = false;
    createOutcomeUnknown = false;
    try {
      const subject = await createSubjektivSubject(fetch, data.workspaceId, {
        role,
        behavior_md: behaviorMd,
      });
      createdSubject = subject;
      try {
        await goto(subjectHref(subject.id));
      } catch {
        createError = 'Subject created, but navigation failed. Open the created Subject below.';
      }
    } catch (error) {
      if (error instanceof SubjektivSubjectCreateError) {
        roleInvalid = error.kind === 'validation' && error.message.startsWith('Subject role');
        behaviorInvalid = error.kind === 'validation' && error.message.startsWith('Subject behavior');
        createOutcomeUnknown = error.kind === 'unknown_outcome';
        createError = error.status === 401 || error.status === 403
          ? 'You do not have permission to create Subjects in this Workspace.'
          : error.message;
        if (roleInvalid || behaviorInvalid) {
          await tick();
          if (behaviorInvalid) behaviorInput?.focus();
          else roleInput?.focus();
        }
      } else {
        createOutcomeUnknown = true;
        createError = 'The request outcome is unknown. A Subject may have been created.';
      }
    } finally {
      submitting = false;
    }
  }

  function createFormKeydown(event: KeyboardEvent): void {
    if (event.key === 'Escape' && !submitting) {
      event.preventDefault();
      void cancelCreate();
    }
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
      <p>Durable context, organized by explicit subject.</p>
    </div>
    <div class="memory-header-actions">
      {#if data.subjects.data}
        <span class="memory-count">{subjects.length}{data.subjects.data.has_more ? '+' : ''} subject{subjects.length === 1 ? '' : 's'}</span>
      {/if}
      <button
        bind:this={createButton}
        type="button"
        class="subject-create-trigger"
        aria-expanded={createExpanded}
        aria-controls="subject-create-panel"
        onclick={() => void openCreateForm()}
      >New Subject</button>
    </div>
  </header>

  {#if createExpanded}
    <form
      id="subject-create-panel"
      class="subject-create-panel"
      aria-labelledby="subject-create-heading"
      aria-busy={submitting}
      onsubmit={(event) => { event.preventDefault(); void submitCreate(); }}
    >
      <div class="subject-create-copy">
        <h2 id="subject-create-heading">Create Subject</h2>
        <p id="subject-role-help">Role describes the Subject’s durable identity. The service assigns its ID; creating it does not start or attach a Worker.</p>
      </div>
      <label class="subject-role-field">
        <span>Role</span>
        <input
          bind:this={roleInput}
          bind:value={role}
          type="text"
          autocomplete="off"
          required
          disabled={submitting || createdSubject !== null}
          onkeydown={createFormKeydown}
          aria-invalid={roleInvalid ? 'true' : undefined}
          aria-describedby={createError && roleInvalid ? 'subject-role-help subject-create-error' : 'subject-role-help'}
        />
      </label>
      <label class="subject-role-field">
        <span>Behavior <small>Optional, user-managed</small></span>
        <textarea
          bind:this={behaviorInput}
          bind:value={behaviorMd}
          rows="7"
          disabled={submitting || createdSubject !== null}
          onkeydown={createFormKeydown}
          aria-invalid={behaviorInvalid ? 'true' : undefined}
          aria-describedby={createError && behaviorInvalid ? 'subject-behavior-help subject-create-error' : 'subject-behavior-help'}
        ></textarea>
        <small id="subject-behavior-help">Persistent guidance injected verbatim for this Subject. It remains separate from generated Memory and can be edited later.</small>
      </label>
      {#if createError}
        <div
          id="subject-create-error"
          class:unknown-outcome={createOutcomeUnknown}
          class="subject-create-error"
          role="alert"
        >
          <p>{createError}</p>
          {#if createOutcomeUnknown}
            <p><a href={pageHref()}>Check the first Subjects page before retrying.</a></p>
          {:else if createdSubject}
            <p><a href={subjectHref(createdSubject.id)}>Open {createdSubject.id}</a></p>
          {/if}
        </div>
      {/if}
      <div class="subject-create-actions">
        <button type="submit" disabled={submitting || createdSubject !== null}>
          {submitting ? 'Creating…' : 'Create Subject'}
        </button>
        <button type="button" class="subject-create-cancel" disabled={submitting} onclick={() => void cancelCreate()}>Cancel</button>
      </div>
    </form>
  {/if}

  {#if data.subjects.data}
    {#if subjects.length === 0}
      <div class="memory-state" role="status" data-memory-state="empty">
        <strong>No Memory subjects.</strong>
        <p>Subjects will appear here after they are created for this Workspace.</p>
        {#if data.cursor}<p><a href={pageHref()}>Return to the first page</a></p>{/if}
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
      <nav class="memory-pagination" aria-label="Subject pages">
        {#if data.cursor}<a href={pageHref()}>First page</a>{/if}
        {#if data.subjects.data.has_more && data.subjects.data.next_cursor}
          <a href={pageHref(data.subjects.data.next_cursor)}>Next page →</a>
        {/if}
      </nav>
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

  .memory-header-actions,
  .subject-create-actions {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--space-3);
  }

  .memory-header-actions {
    justify-content: flex-end;
  }

  .subject-create-trigger,
  .subject-create-actions button {
    min-height: 2.25rem;
    border: 0;
    border-radius: var(--radius-soft);
    padding: var(--space-2) var(--space-4);
    background: var(--accent);
    color: var(--bg);
    font-weight: 700;
    cursor: pointer;
  }

  .subject-create-trigger:hover,
  .subject-create-trigger:focus-visible,
  .subject-create-actions button:hover:not(:disabled),
  .subject-create-actions button:focus-visible:not(:disabled) {
    filter: brightness(1.08);
  }

  .subject-create-actions button:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .subject-create-actions .subject-create-cancel {
    border: 1px solid var(--line);
    background: transparent;
    color: var(--text-strong);
  }

  .subject-create-actions {
    grid-column: 2;
  }

  .subject-create-panel {
    display: grid;
    grid-template-columns: minmax(14rem, 0.7fr) minmax(18rem, 1.3fr);
    align-items: start;
    gap: var(--space-4);
    padding: var(--space-4);
    background: var(--bg-raised);
    border-radius: var(--radius-soft);
  }

  .subject-create-copy {
    grid-column: 1;
    grid-row: 1 / span 4;
    align-self: start;
  }

  .subject-create-copy h2,
  .subject-create-copy p,
  .subject-create-error p {
    margin: 0;
  }

  .subject-create-copy p,
  .subject-create-error {
    margin-top: var(--space-1);
    color: var(--text-muted);
    font-size: var(--font-size-compact);
  }

  .subject-role-field {
    display: grid;
    gap: var(--space-1);
    color: var(--text-muted);
    font-size: var(--font-size-compact);
    font-weight: 700;
  }

  .subject-role-field input,
  .subject-role-field textarea {
    width: 100%;
    min-width: 0;
    border: 1px solid var(--line);
    border-radius: var(--radius-soft);
    padding: var(--space-2) var(--space-3);
    background: var(--bg);
    color: var(--text-strong);
  }

  .subject-role-field textarea {
    min-height: 8rem;
    resize: vertical;
    font: inherit;
    line-height: 1.5;
  }

  .subject-role-field input[aria-invalid='true'],
  .subject-role-field textarea[aria-invalid='true'] {
    border-color: var(--danger);
  }

  .subject-create-error {
    grid-column: 2 / -1;
    color: var(--danger);
  }

  .subject-create-error a {
    color: inherit;
  }

  .subject-create-error.unknown-outcome {
    color: var(--warning);
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

  .memory-pagination {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: var(--space-3);
    font-size: var(--font-size-compact);
  }

  .memory-state p {
    margin: var(--space-1) 0 0;
    color: var(--text-muted);
  }

  .memory-state.is-error,
  .memory-state.is-error strong {
    color: var(--danger);
  }

  @media (max-width: 900px) {
    .subject-create-panel {
      grid-template-columns: minmax(0, 1fr);
      align-items: stretch;
    }

    .subject-create-copy,
    .subject-create-error,
    .subject-create-actions {
      grid-column: 1;
      grid-row: auto;
    }

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

    .memory-header-actions {
      justify-content: flex-start;
    }

    .subject-create-panel {
      padding: var(--space-3);
    }

    .subject-row {
      padding-inline: 0;
    }

    .subject-meta {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
