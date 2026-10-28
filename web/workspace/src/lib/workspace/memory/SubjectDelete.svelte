<script lang="ts">
  import { untrack } from 'svelte';
  import { deleteSubjektivSubject, SubjektivSubjectDeleteError } from './api.ts';
  import { clearSubjectDeleteAttempt, readSubjectDeleteAttempt, saveSubjectDeleteAttempt, type SubjectDeleteAttempt } from './subject-delete.ts';
  import type { SubjektivSubjectResponse } from '#lib/generated/memory-api.ts';
  import { workspaceRoute } from '#lib/workspace/api/http.ts';

  let { workspaceId, subjectId, subject, onstarted }: {
    workspaceId: string;
    subjectId: string;
    subject: SubjektivSubjectResponse | null;
    onstarted: () => void;
  } = $props();
  let attempt = $state<SubjectDeleteAttempt | null>(null);
  let phase = $state<'idle' | 'confirming' | 'running' | 'pending' | 'unknown' | 'failed' | 'completed'>('idle');
  let confirmed = $state(false);
  let error = $state<string | null>(null);
  let storageError = $state(false);
  let identity = '';
  let generation = 0;

  $effect(() => {
    const next = JSON.stringify([workspaceId, subjectId]);
    untrack(() => {
      if (identity === next) return;
      identity = next;
      generation += 1;
      attempt = null;
      phase = 'idle';
      confirmed = false;
      error = null;
      storageError = false;
      try {
        attempt = readSubjectDeleteAttempt(sessionStorage, workspaceId, subjectId);
        if (attempt) {
          phase = 'unknown';
          onstarted();
        }
      } catch (cause) {
        storageError = true;
        error = cause instanceof Error ? cause.message : 'Cannot read the saved deletion request. Do not start a new request.';
      }
    });
  });

  async function submit(): Promise<void> {
    if (phase === 'running' || phase === 'completed' || storageError || (!attempt && (!subject || !confirmed))) return;
    const currentGeneration = generation;
    const targetWorkspace = workspaceId;
    const targetSubject = subjectId;
    const request = attempt ?? { subjectId: targetSubject, operationId: crypto.randomUUID() };
    const current = () => currentGeneration === generation && targetWorkspace === workspaceId && targetSubject === subjectId;
    // 保存に失敗したら送らない。応答喪失後に新しいkeyを作る経路を残さない。
    try { saveSubjectDeleteAttempt(sessionStorage, targetWorkspace, request); }
    catch {
      error = 'The retry identity could not be saved in this tab. No deletion request was sent. Allow session storage and try again.';
      return;
    }
    attempt = request;
    phase = 'running';
    error = null;
    onstarted();
    try {
      const result = await deleteSubjektivSubject(fetch, targetWorkspace, targetSubject, { operation_id: request.operationId });
      // 対象変更後の遅延応答は新対象を更新せず、成功した旧対象の再送入力だけ消去する。
      if (result.completed) {
        try { clearSubjectDeleteAttempt(sessionStorage, targetWorkspace, targetSubject); }
        catch { /* 消去できなくても残るのは同一keyだけ。本文や成功状態は保存しない。 */ }
      }
      if (!current()) return;
      phase = result.completed ? 'completed' : 'pending';
    } catch (cause) {
      if (!current()) return;
      phase = cause instanceof SubjektivSubjectDeleteError && cause.kind === 'rejected' ? 'failed' : 'unknown';
      error = cause instanceof Error ? cause.message : 'The deletion outcome is unknown.';
    }
  }
</script>

{#if subject || attempt || storageError}
  <section class="subject-deletion" aria-labelledby="subject-delete-heading" data-delete-state={phase}>
    <header>
      <h2 id="subject-delete-heading">Delete Subject</h2>
      {#if phase === 'idle' && !storageError}
        <button type="button" class="workspace-secondary-button subject-delete-open" onclick={() => { phase = 'confirming'; }}>Delete Subject…</button>
      {/if}
    </header>
    {#if phase === 'confirming' || attempt}
      <p class="delete-target">Target: <strong>{subject?.role ?? subjectId}</strong> · <code>{subjectId}</code></p>
    {/if}
    {#if phase === 'confirming'}
      <form onsubmit={(event) => { event.preventDefault(); void submit(); }}>
        <p><strong>This is irreversible.</strong> This Subject, its behavior, Memory, change history, staging, Surface, Journal, permissions, delegations, Writer/Session associations, and Subject Job inputs/results will be deleted. Connected and delegated Workers and Jobs must stop before completion can be confirmed.</p>
        <p>Original Worker/Session text, existing archives and Workdirs, Tickets, and other separately retained resources are <strong>not deleted</strong>. Minimal deletion retry/audit metadata is retained. Other Subjects and Workspaces are not affected.</p>
        <label class="delete-confirm"><input type="checkbox" bind:checked={confirmed} /> I have checked the target, deletion scope, retained data, and irreversible effect.</label>
        <div class="delete-actions">
          <button type="submit" class="workspace-danger-button" disabled={!confirmed}>Permanently delete Subject</button>
          <button type="button" class="workspace-secondary-button" onclick={() => { phase = 'idle'; confirmed = false; }}>Cancel</button>
        </div>
      </form>
    {:else if phase === 'running'}
      <p role="status">Deleting… Waiting for stop and erasure confirmation. Do not treat this as completed.</p>
    {:else if phase === 'pending'}
      <p role="status"><strong>Deletion started; completion unconfirmed.</strong> The deletion fence is saved and the Subject is hidden from reads/lists. External stopping or erasure is not yet confirmed. Retry the same request to continue.</p>
    {:else if phase === 'unknown'}
      <p role="status"><strong>Deletion outcome unknown.</strong> The request may already have taken effect. The saved target and operation ID are retained; retry the same request, not a new deletion.</p>
    {:else if phase === 'failed'}
      <p role="alert"><strong>Deletion request rejected.</strong> Completion is not confirmed. Resolve the reported condition, then retry the same request.</p>
    {:else if phase === 'completed'}
      <p role="status"><strong>Subject deletion completed.</strong> Backend confirmed stopping and deletion of Subject-owned data. Separately retained Worker/Session text, archives, Workdirs, and Tickets remain.</p>
      <a href={workspaceRoute(workspaceId, '/memory')}>Back to subjects →</a>
    {/if}
    {#if error}<p class="workspace-callout is-error" role="alert">{error}</p>{/if}
    {#if attempt && phase !== 'completed'}
      <details><summary>Deletion request identity</summary><code>{attempt.operationId}</code><p>This tab retains the target and same request ID across reloads, even when GET can no longer find the Subject.</p></details>
      {#if phase === 'pending' || phase === 'unknown' || phase === 'failed'}
        <button type="button" class="workspace-secondary-button subject-delete-retry" onclick={() => { void submit(); }}>Retry same deletion request</button>
      {/if}
    {/if}
  </section>
{/if}

<style>
  .subject-deletion { min-width: 0; }
  header { display: flex; align-items: center; justify-content: space-between; gap: .75rem; flex-wrap: wrap; }
  h2 { font-size: var(--font-size-body); margin: 0; }
  p { line-height: 1.6; }
  code, .delete-target { overflow-wrap: anywhere; }
  .delete-confirm { display: flex; align-items: flex-start; gap: .6rem; line-height: 1.5; }
  input { margin-top: .25rem; flex: 0 0 auto; }
  .delete-actions { display: flex; gap: .75rem; flex-wrap: wrap; margin-top: 1rem; }
  details { margin-block: 1rem; }
  summary { cursor: pointer; }
</style>
