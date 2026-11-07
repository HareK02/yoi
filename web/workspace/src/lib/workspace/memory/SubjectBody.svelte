<script lang="ts">
  import { untrack } from 'svelte';
  import type { SubjectBodyMutationResponse, SubjectBodyResponse, SubjectBodyStopOutcomeResponse } from '#lib/generated/subject-body-api.ts';
  import { liveWorkerState } from '#lib/workspace/sidebar/worker-state.ts';
  import {
    clearSubjectBodyAttempt, createSubjectBodyAttempt, getSubjectBody, getSubjectBodyStopOutcome,
    readSubjectBodyAttempt, saveSubjectBodyAttempt, startSubjectBody,
    recoverSubjectBody, stopSubjectBody, subjectBodyControls, SubjectBodyApiError, type SubjectBodyAttempt,
  } from './subject-body.ts';

  let { accountId, workspaceId, subjectId }: { accountId: string; workspaceId: string; subjectId: string } = $props();
  const headingId = $props.id();
  let body = $state.raw<SubjectBodyResponse | null>(null);
  let attempt = $state.raw<SubjectBodyAttempt | null>(null);
  let receipt = $state.raw<SubjectBodyMutationResponse | null>(null);
  let stopOutcome = $state.raw<SubjectBodyStopOutcomeResponse | null>(null);
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
  let worker = $derived(snapshot?.observation.kind === 'available' ? snapshot.observation.worker : null);
  let workerDiagnostics = $derived(worker?.diagnostics ?? []);

  function current(generation: number, account: string, workspace: string, subject: string): boolean {
    return routeGeneration === generation && accountId === account && workspaceId === workspace && subjectId === subject;
  }

  $effect(() => {
    const account = accountId;
    const workspace = workspaceId;
    const subject = subjectId;
    untrack(() => {
      routeGeneration += 1;
      observationRequest += 1;
      activeRoute = identityKey(account, workspace, subject);
      body = null;
      attempt = null;
      receipt = null;
      stopOutcome = null;
      outcomeError = null;
      checkingOutcome = false;
      loadError = null;
      operationError = null;
      storageError = null;
      sending = false;
      loading = false;
      fresh = false;
      if (typeof account !== 'string' || account.length === 0) return;
      try { attempt = readSubjectBodyAttempt(sessionStorage, account, workspace, subject); }
      catch { storageError = 'The saved body request cannot be read. Do not create a replacement request. Restore access to this tab’s session storage and reload.'; }
      void refresh();
    });
    return () => { routeGeneration += 1; observationRequest += 1; };
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
      fresh = true;
    } catch (cause) {
      if (!latest()) return;
      loadError = (cause instanceof SubjectBodyApiError ? `${cause.message}. ` : '') + 'Body state could not be confirmed. Check your connection and viewing permissions, then refresh.';
    } finally {
      if (latest()) loading = false;
    }
  }

  function settleFailedAttempt(cause: unknown, frozen: SubjectBodyAttempt): string | null {
    if (!(cause instanceof SubjectBodyApiError) || !cause.request || (cause.request.resolution !== 'not_accepted' && cause.request.resolution !== 'superseded')) return null;
    const rejected = cause.request;
    if (rejected.workspace_id !== frozen.workspaceId || rejected.subject_id !== frozen.subjectId || rejected.request_id !== frozen.request.request_id) return null;
    receipt = null;
    stopOutcome = null;
    try {
      clearSubjectBodyAttempt(sessionStorage, accountId, frozen);
      attempt = null;
    } catch {
      storageError = 'The request rejection was confirmed, but its saved input could not be cleared. Restore session storage access and reload before another action.';
    }
    return (rejected.resolution === 'superseded'
      ? 'Request superseded. The body target changed, so this saved intent can no longer act on the current body. This does not determine whether an earlier operation took effect. '
      : 'Request not accepted. The saved observation was stale and this request was not admitted. ')
      + cause.message + '. Refresh the current body before choosing a new explicit action.';
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
    stopOutcome = null;
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
    stopOutcome = null;
    try {
      const result = await getSubjectBodyStopOutcome(fetch, workspace, subject, frozen.request);
      if (!current(generation, account, workspace, subject)) return;
      stopOutcome = result;
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

<section class="subject-body" aria-labelledby={headingId}>
  <header>
    <h2 id={headingId}>Conversation body</h2>
    {#if hasAccount}<button type="button" class="workspace-secondary-button" disabled={!visible || loading} onclick={() => { void refresh(); }}>Refresh body state</button>{/if}
  </header>
  {#if !hasAccount}
    <p role="status">Sign in to view this conversation body.</p>
  {:else if !visible || (!snapshot && loading)}
    <p role="status">Loading body state…</p>
  {:else if snapshot}
    {#if !fresh}<p class="muted">{loading ? 'Updating body state…' : 'Last confirmed observation; refresh before acting.'}</p>{/if}
    {#if snapshot.observation.kind === 'absent'}
      <p>No current conversation body.</p>
    {:else if snapshot.observation.kind === 'failed_create'}
      <p><strong>Body creation failed; reservation retained.</strong> Confirm Runtime cleanup to release this reservation. This does not start a new body or change Subject permissions.</p>
    {:else if snapshot.observation.kind === 'unavailable'}
      <p><strong>Body owner present; observation unavailable.</strong> This is not an absent or stopped body. A replacement will not be started.</p>
      {#each snapshot.observation.diagnostics as diagnostic}
        <p class="workspace-callout" role="status">{diagnostic.message}</p>
      {/each}
    {:else if worker}
      {#if worker.availability !== 'observed'}
        <p><strong>Body owner present; Worker observation unavailable.</strong> Running or stopped state is not confirmed.</p>
      {:else if liveWorkerState(worker) === 'stopped'}
        <p><strong>Body stopped.</strong> Start restores this body; retained conversation and Memory remain available.</p>
      {:else if worker.worker_state}
        <dl><div><dt>Worker lifecycle</dt><dd>{worker.state}</dd></div><div><dt>Live execution</dt><dd>{liveWorkerState(worker)}</dd></div></dl>
      {:else}
        <p><strong>Body owner present; live execution not confirmed.</strong> Refresh before acting.</p>
      {/if}
      {#if worker.availability === 'observed' && !worker.restore_observation_token}
        <p class="muted">Lifecycle controls are unavailable until the current body’s observation token can be confirmed. Refresh body state before acting.</p>
      {/if}
      {#each workerDiagnostics as diagnostic}
        <p class="workspace-callout" role="status">{diagnostic.message}</p>
      {/each}
    {/if}
    {#if !snapshot.can_start || !snapshot.can_stop}
      <p class="muted">{!snapshot.can_start && !snapshot.can_stop ? 'You do not have permission to start or stop this body.' : !snapshot.can_start ? 'You do not have permission to start this body.' : 'You do not have permission to stop this body.'} Ask a Workspace administrator to review access; this control does not add permissions.</p>
    {/if}
    {#if !attempt}
      <div class="actions">
        {#if controls.start}
          <button type="button" class="workspace-secondary-button" disabled={!fresh || loading || sending || !!storageError} onclick={() => { void submit('start'); }}>{snapshot.current ? 'Restart conversation body' : 'Start conversation body'}</button>
        {/if}
        {#if controls.recover}
          <button type="button" class="workspace-secondary-button" disabled={!fresh || loading || sending || !!storageError} onclick={() => { void submit('recover'); }}>Release failed start reservation</button>
        {/if}
        {#if controls.stop}
          <button type="button" class="workspace-secondary-button" disabled={!fresh || loading || sending || !!storageError} onclick={() => { void submit('stop'); }}>Stop conversation body</button>
        {/if}
      </div>
      {#if controls.stop}<p class="muted">Stopping ends the body Worker, not the Subject. Conversation and Memory are retained.</p>{/if}
    {/if}
  {/if}
  {#if visible}
    {#if loadError}<p class="workspace-callout is-error" role="alert">{loadError}</p>{/if}
    {#if storageError}<p class="workspace-callout is-error" role="alert">{storageError}</p>{/if}
    {#if sending}<p role="status">Sending request; body state is not yet confirmed.</p>{/if}
    {#if receipt}
      <div role="status">
        {#if receipt.disposition === 'accepted'}
          <p><strong>Request accepted.</strong> Acceptance is not confirmation of running or stopped state. Check the body observation above.</p>
        {:else if receipt.disposition === 'rejected'}
          <p><strong>Request rejected.</strong> Review the diagnostics and refresh before a new action. Check Subject permissions, the default profile and model configuration; no permissions were added.</p>
        {:else}
          <p><strong>Request outcome unknown.</strong> {attempt?.action === 'stop' ? 'Check the saved stop outcome without sending another stop, or explicitly retry the same saved request.' : 'Refresh observations, then retry the same saved request to reconcile.'} Refresh alone does not settle this receipt.</p>
        {/if}
        {#each receipt.diagnostics as diagnostic}<p class="workspace-callout">{diagnostic.message}</p>{/each}
      </div>
    {/if}
    {#if checkingOutcome}<p role="status">Checking saved stop outcome… No stop request is sent.</p>{/if}
    {#if stopOutcome}
      <div role="status">
        {#if stopOutcome.outcome === 'stopped'}
          <p><strong>Historical stop confirmed.</strong> This settles the saved stop operation, not the current body’s state. The body may have been restored since then; see the current observation above.</p>
        {:else if stopOutcome.outcome === 'pending'}
          <p><strong>Stop outcome pending.</strong> Stopping is not confirmed. The complete saved request is retained; check again or explicitly retry the same request.</p>
        {:else}
          <p><strong>No stop receipt found.</strong> This does not prove that stopping failed or that the current body is stopped. The complete saved request is retained; check again or explicitly retry the same request.</p>
        {/if}
      </div>
    {/if}
    {#if outcomeError}<p class="workspace-callout is-error" role="alert">{outcomeError}</p>{/if}
    {#if operationError}<p class="workspace-callout is-error" role="alert">{operationError}</p>{/if}
    {#if attempt && !sending}
      {#if !receipt && !operationError && stopOutcome?.outcome !== 'stopped'}<p role="status">An unresolved {attempt.action} request is retained in this tab. Refresh and retry the same request; do not create a replacement.</p>{/if}
      {#if attempt.action === 'stop' && snapshot?.can_stop}
        <button type="button" class="workspace-secondary-button" disabled={checkingOutcome || !!storageError} onclick={() => { void checkStopOutcome(); }}>Check stop outcome</button>
      {/if}
      {#if retryAuthorized}
        <button type="button" class="workspace-secondary-button" disabled={!fresh || loading || checkingOutcome || !!storageError} onclick={() => { void submit(); }}>Retry same {attempt.action} request</button>
      {/if}
      <details><summary>Request diagnostics</summary><p>Request ID: <code>{attempt.request.request_id}</code></p><p>The complete frozen request is retained across reloads in this tab. Retrying does not use a new request ID or a newer observation token.</p></details>
    {/if}
  {/if}
</section>

<style>
  .subject-body { min-width: 0; overflow-wrap: anywhere; }
  header, .actions { display: flex; align-items: center; flex-wrap: wrap; gap: var(--space-3); }
  header { justify-content: space-between; }
  h2 { margin: 0; font-size: var(--font-size-body); line-height: var(--line-height-body); }
  p, dl, details { margin-block: var(--space-3); }
  .muted, dt { color: var(--text-muted); }
  dl > div { display: flex; flex-wrap: wrap; gap: var(--space-3); }
  dd { margin: 0; }
  code { font-family: var(--font-mono); }
  summary { cursor: pointer; }
  button { font-size: var(--font-size-body); line-height: var(--line-height-body); border-radius: var(--radius-soft); }
  button:hover:not(:disabled) { background: var(--interactive-hover); }
  button:active:not(:disabled) { background: var(--interactive-selected); }
  button:focus-visible, summary:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>
