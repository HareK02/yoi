<script lang="ts">
  import { untrack } from 'svelte';
  import type { SubjectBodyMutationResponse, SubjectBodyResponse } from '#lib/generated/subject-body-api.ts';
  import { workspaceMultiplexer } from '#lib/workspace/multiplexer.ts';
  import {
    clearSubjectBodyAttempt, createSubjectBodyAttempt, getSubjectBody, getSubjectBodyStopOutcome,
    readSubjectBodyAttempt, saveSubjectBodyAttempt, startSubjectBody,
    recoverSubjectBody, stopSubjectBody, subjectBodyControls, SubjectBodyApiError, type SubjectBodyAttempt,
  } from './subject-body.ts';

  let { accountId, workspaceId, subjectId }: { accountId: string; workspaceId: string; subjectId: string } = $props();
  let body = $state.raw<SubjectBodyResponse | null>(null);
  let attempt = $state.raw<SubjectBodyAttempt | null>(null);
  let receipt = $state.raw<SubjectBodyMutationResponse | null>(null);
  let outcomeError = $state<string | null>(null);
  let checkingOutcome = $state(false);
  let loadError = $state<string | null>(null);
  let operationError = $state<string | null>(null);
  let storageError = $state<string | null>(null);
  let loading = $state(false);
  let sending = $state(false);
  let fresh = $state(false);
  let activeRoute = $state('');
  let routeGeneration = 0;
  let observationRequest = 0;
  const identityKey = (account: string, workspace: string, subject: string) => JSON.stringify([account, workspace, subject]);
  let hasAccount = $derived(typeof accountId === 'string' && accountId.length > 0);
  let visible = $derived(hasAccount && activeRoute === identityKey(accountId, workspaceId, subjectId));
  let snapshot = $derived(visible ? body : null);
  let controls = $derived(snapshot ? subjectBodyControls(snapshot) : { start: false, stop: false, recover: false });
  let retryAuthorized = $derived(snapshot && attempt && (attempt.action === 'stop' ? snapshot.can_stop : snapshot.can_start));
  let updatesUnavailable = $state(false);

  function current(generation: number, account: string, workspace: string, subject: string): boolean {
    return routeGeneration === generation && accountId === account && workspaceId === workspace && subjectId === subject;
  }

  $effect(() => {
    const account = accountId;
    const workspace = workspaceId;
    const subject = subjectId;
    const stop = untrack(() => {
      routeGeneration += 1;
      observationRequest += 1;
      activeRoute = identityKey(account, workspace, subject);
      body = null;
      attempt = null;
      receipt = null;
      outcomeError = null;
      checkingOutcome = false;
      loadError = null;
      operationError = null;
      storageError = null;
      sending = false;
      loading = false;
      fresh = false;
      updatesUnavailable = false;
      if (typeof account !== 'string' || account.length === 0) return;
      try { attempt = readSubjectBodyAttempt(sessionStorage, account, workspace, subject); }
      catch { storageError = 'Could not read the operation. Check browser storage before trying again.'; }
      const generation = routeGeneration;
      let disposed = false;
      let queued = false;
      const invalidate = () => {
        if (disposed || !current(generation, account, workspace, subject)) return;
        fresh = false;
        observationRequest += 1;
        if (queued) return;
        queued = true;
        queueMicrotask(() => {
          queued = false;
          if (!disposed && current(generation, account, workspace, subject)) void refresh();
        });
      };
      // Worker通知は再取得の契機だけに使い、Subjectの所有・操作権限を推定しない。
      const subscription = workspaceMultiplexer(workspace).subscribe({ topic: 'workspace_workers' }, {
        onFrame(frame) {
          if (disposed || !current(generation, account, workspace, subject)) return;
          if ((frame.frame === 'response' && frame.message.result === 'subscribed') ||
              (frame.frame === 'event' && frame.message.event === 'event')) {
            updatesUnavailable = false;
            invalidate();
          } else if ((frame.frame === 'response' && frame.message.result === 'subscription_rejected') ||
                     (frame.frame === 'event' && frame.message.event === 'subscription_closed')) {
            updatesUnavailable = true;
            fresh = false;
            loading = false;
            observationRequest += 1;
          }
        },
        onStatus(status) {
          if (disposed || !current(generation, account, workspace, subject)) return;
          if (status !== 'open') {
            updatesUnavailable = true;
            fresh = false;
            loading = false;
            observationRequest += 1;
          }
        },
      });
      void refresh();
      return () => { disposed = true; subscription.close(); };
    });
    return () => { stop?.(); routeGeneration += 1; observationRequest += 1; };
  });

  async function refresh(): Promise<void> {
    if (!visible) return;
    const generation = routeGeneration;
    const account = accountId;
    const workspace = workspaceId;
    const subject = subjectId;
    const request = ++observationRequest;
    const latest = () => current(generation, account, workspace, subject) && request === observationRequest;
    loading = true;
    fresh = false;
    loadError = null;
    try {
      const next = await getSubjectBody(fetch, workspace, subject);
      if (!latest()) return;
      body = next;
      fresh = !updatesUnavailable;
    } catch {
      if (!latest()) return;
      loadError = 'Subject controls are temporarily unavailable.';
    } finally {
      if (latest()) loading = false;
    }
  }

  function settleFailedAttempt(cause: unknown, frozen: SubjectBodyAttempt): string | null {
    if (!(cause instanceof SubjectBodyApiError) || !cause.request || (cause.request.resolution !== 'not_accepted' && cause.request.resolution !== 'superseded')) return null;
    const rejected = cause.request;
    if (rejected.workspace_id !== frozen.workspaceId || rejected.subject_id !== frozen.subjectId || rejected.request_id !== frozen.request.request_id) return null;
    receipt = null;
    try {
      clearSubjectBodyAttempt(sessionStorage, accountId, frozen);
      attempt = null;
    } catch {
      storageError = 'The request rejection was confirmed, but its saved input could not be cleared. Restore session storage access and reload before another action.';
    }
    return 'The Subject changed. Try again.';
  }

  async function submit(action?: 'start' | 'stop' | 'recover'): Promise<void> {
    if (!visible || sending || checkingOutcome || loading || storageError || !fresh || !snapshot) return;
    if (attempt ? !retryAuthorized : (!action || !controls[action])) return;
    const generation = routeGeneration;
    const account = accountId;
    const workspace = workspaceId;
    const subject = subjectId;
    let frozen: SubjectBodyAttempt;
    try {
      frozen = attempt ?? createSubjectBodyAttempt(account, snapshot, action!, crypto.randomUUID());
      // 送信より先に全入力を保存する。保存不能・既存の別依頼は送らずに止める。
      saveSubjectBodyAttempt(sessionStorage, account, frozen);
    } catch {
      storageError = 'The exact request could not be retained in this tab. No request was sent. Restore session storage access and reload before trying again.';
      return;
    }
    attempt = frozen;
    sending = true;
    operationError = null;
    receipt = null;
    outcomeError = null;
    fresh = false;
    const observation = ++observationRequest;
    try {
      const result = frozen.action === 'start'
        ? await startSubjectBody(fetch, workspace, subject, frozen.request)
        : frozen.action === 'recover'
        ? await recoverSubjectBody(fetch, workspace, subject, frozen.request)
        : await stopSubjectBody(fetch, workspace, subject, frozen.request);
      if (!current(generation, account, workspace, subject)) return;
      receipt = result;
      // receiptのacceptedは実行状態ではない。bodyがあれば独立した観測として扱う。
      if (result.body && observation === observationRequest) body = result.body;
      if (result.disposition !== 'unknown') {
        try {
          clearSubjectBodyAttempt(sessionStorage, account, frozen);
          attempt = null;
        } catch {
          storageError = 'The request receipt was received, but its saved retry input could not be cleared. Reload to reconcile the retained request; do not create another request.';
        }
      }
    } catch (cause) {
      if (!current(generation, account, workspace, subject)) return;
      operationError = settleFailedAttempt(cause, frozen) ?? ((cause instanceof SubjectBodyApiError ? `${cause.message}. ` : '') + 'Request outcome unknown. The request may already have taken effect. Its exact input is retained; refresh observations and explicitly retry the same request to reconcile.');
    } finally {
      if (current(generation, account, workspace, subject)) {
        sending = false;
        // GET更新だけではunknownの再送入力を消さない。
        void refresh();
      }
    }
  }
  async function checkStopOutcome(): Promise<void> {
    if (!visible || sending || checkingOutcome || storageError || !snapshot?.can_stop || attempt?.action !== 'stop' || attempt.accountId !== accountId) return;
    const generation = routeGeneration;
    const account = accountId;
    const workspace = workspaceId;
    const subject = subjectId;
    const frozen = attempt;
    checkingOutcome = true;
    outcomeError = null;
    try {
      const result = await getSubjectBodyStopOutcome(fetch, workspace, subject, frozen.request);
      if (!current(generation, account, workspace, subject)) return;
      // 停止照会のbodyは表示状態へ転記しない。現在の本体は独立したGETで更新する。
      if (result.outcome === 'stopped') {
        receipt = null;
        operationError = null;
        try {
          clearSubjectBodyAttempt(sessionStorage, account, frozen);
          attempt = null;
        } catch {
          storageError = 'The historical stop was confirmed, but its saved retry input could not be cleared. Reload to reconcile the retained request; do not create another request.';
        }
      }
      // not_found/pendingとGET更新のいずれも、凍結した停止依頼を消さない。
      void refresh();
    } catch (cause) {
      if (!current(generation, account, workspace, subject)) return;
      const settled = settleFailedAttempt(cause, frozen);
      outcomeError = settled ?? ((cause instanceof SubjectBodyApiError ? `${cause.message}. ` : '') + 'Stop outcome could not be checked. This is not a missing receipt or a confirmed stop. The exact request is retained; try the read-only check again.');
      if (settled) { operationError = null; void refresh(); }
    } finally {
      if (current(generation, account, workspace, subject)) checkingOutcome = false;
    }
  }
</script>

<section class="subject-body" aria-label="Subject controls" aria-busy={loading || sending || checkingOutcome}>
  {#if !hasAccount}
    <p role="status">Sign in to use Subject controls.</p>
  {:else if visible}
    {#if sending || checkingOutcome}
      <p role="status">Working…</p>
    {:else if storageError}
      <p role="alert">Could not save the operation. Check browser storage before trying again.</p>
    {:else if loadError || updatesUnavailable || snapshot?.observation.kind === "unavailable"}
      <p role="status">Subject controls are temporarily unavailable.</p>
    {:else if operationError === "The Subject changed. Try again." || outcomeError === "The Subject changed. Try again."}
      <p role="alert">The Subject changed. Try again.</p>
    {:else if attempt}
      <p role="status">Could not confirm the operation.</p>
    {:else if receipt?.disposition === "rejected"}
      <p role="alert">Could not complete the operation.</p>
    {:else if !snapshot && loading}
      <p role="status">Loading…</p>
    {:else if snapshot && !snapshot.can_start && !snapshot.can_stop}
      <p role="status">Subject controls are unavailable.</p>
    {/if}
    <div class="actions">
      {#if attempt}
        {#if attempt.action === "stop" && snapshot?.can_stop}
          <button type="button" class="workspace-secondary-button" disabled={loading || sending || checkingOutcome || !!storageError} onclick={() => { void checkStopOutcome(); }}>Check result</button>
        {/if}
        {#if retryAuthorized}
          <button type="button" class="workspace-secondary-button" disabled={!fresh || loading || sending || checkingOutcome || !!storageError} onclick={() => { void submit(); }}>Retry</button>
        {/if}
      {:else if snapshot}
        {#if controls.start}
          <button type="button" class="workspace-secondary-button" disabled={!fresh || loading || sending || !!storageError} onclick={() => { void submit("start"); }}>Start Subject</button>
        {/if}
        {#if controls.recover}
          <button type="button" class="workspace-secondary-button" disabled={!fresh || loading || sending || !!storageError} onclick={() => { void submit("recover"); }}>Recover start</button>
        {/if}
        {#if controls.stop}
          <button type="button" class="workspace-secondary-button" disabled={!fresh || loading || sending || !!storageError} onclick={() => { void submit("stop"); }}>Stop Subject</button>
        {/if}
      {/if}
    </div>
  {/if}
</section>

<style>
  .subject-body { min-width: 0; overflow-wrap: anywhere; }
  .actions { display: flex; align-items: center; flex-wrap: wrap; gap: var(--space-2); }
  p { margin: 0 0 var(--space-2); }
  [role="alert"] { color: var(--danger); }
  button { font-size: var(--font-size-body); line-height: var(--line-height-body); border-radius: var(--radius-soft); }
  button:hover:not(:disabled) { background: var(--interactive-hover); }
  button:active:not(:disabled) { background: var(--interactive-selected); }
  button:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>
