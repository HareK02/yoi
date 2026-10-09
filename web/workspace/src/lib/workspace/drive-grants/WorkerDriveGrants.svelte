<script lang="ts">
  import { untrack } from 'svelte';
  import type { Worker } from '#lib/workspace/sidebar/types.ts';
  import { createDriveGrant, DriveGrantError, listDriveGrants, revokeDriveGrant, type DriveAccess, type DriveGrantResponse } from './api.ts';

  export type GrantWorker = Pick<Worker, 'runtime_id' | 'worker_id' | 'display_name' | 'label'>;
  let { workspaceId, workers, canManage, workersReady = true }: {
    workspaceId: string; workers: GrantWorker[]; canManage: boolean; workersReady?: boolean;
  } = $props();
  let selected = $state('');
  let access = $state<DriveAccess>('read_only');
  let grants = $state<DriveGrantResponse[]>([]);
  let loading = $state(false);
  let ready = $state(false);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let unresolved = $state(false);
  let showCreate = $state(false);
  let generation = 0;
  let lastWorkspaceId: string | undefined;
  const key = (worker: GrantWorker) => JSON.stringify([worker.runtime_id, worker.worker_id]);
  const selectedWorker = $derived(workers.find((worker) => key(worker) === selected));
  const workerPresent = $derived(workers.some((worker) => key(worker) === selected));
  const identityScope = $derived(JSON.stringify([workspaceId, canManage, selected, workerPresent]));
  const active = $derived(selectedWorker ? grants.filter((grant) => !grant.revoked && grant.workspace_id === workspaceId && grant.runtime_id === selectedWorker.runtime_id && grant.worker_id === selectedWorker.worker_id) : []);

  function scope() {
    const id = workspaceId, identity = selected, epoch = generation;
    return { id, current: () => generation === epoch && workspaceId === id && selected === identity && canManage && workers.some((worker) => key(worker) === identity) };
  }
  // Workspace, Worker membership and permission define identity. Catalog readiness
  // only gates new actions: it must not abandon a transmitted mutation on the same Worker.
  $effect(() => {
    identityScope;
    untrack(() => {
      generation++;
      grants = []; ready = false; busy = false; loading = false;
      error = null; notice = null; unresolved = false; showCreate = false; access = 'read_only';
      if (lastWorkspaceId !== workspaceId) { lastWorkspaceId = workspaceId; selected = ''; }
      if (!workerPresent && selected) selected = '';
      if (workspaceId && canManage && workersReady && workerPresent) void refresh();
    });
    return () => { generation++; };
  });

  $effect(() => {
    const usable = workersReady;
    identityScope;
    untrack(() => {
      if (usable && canManage && workerPresent && !busy && !loading && !ready && !unresolved) void refresh();
    });
  });

  async function refresh() {
    if (!canManage || !workersReady || !selectedWorker || busy || loading) return;
    const request = scope();
    loading = true; ready = false; error = null;
    try {
      const result = await listDriveGrants(request.id);
      if (!request.current()) return;
      grants = result; ready = true; unresolved = false;
    } catch (cause) {
      if (request.current()) error = cause instanceof Error ? cause.message : 'Drive grants could not be loaded.';
    } finally { if (request.current()) loading = false; }
  }
  async function mutate(grant?: DriveGrantResponse) {
    const worker = selectedWorker;
    if (!worker || !canManage || !workersReady || !ready || loading || busy || unresolved) return;
    if (!grant && active.length > 0) return;
    if (grant && !active.some((item) => item.grant_id === grant.grant_id)) return;
    const request = scope();
    const requestedAccess = access;
    busy = true; error = null; notice = null;
    try {
      const result = grant
        ? await revokeDriveGrant(request.id, grant)
        : await createDriveGrant(request.id, { runtime_id: worker.runtime_id, worker_id: worker.worker_id, access: requestedAccess });
      if (!request.current()) return;
      grants = [...grants.filter((item) => item.grant_id !== result.grant_id), result];
      notice = grant ? 'Drive grant revoked.' : 'Drive grant created.';
      showCreate = false;
      // Re-read current authority, rather than treating the mutation response as a lease.
      busy = false; ready = false;
      await refresh();
    } catch (cause) {
      if (!request.current()) return;
      unresolved = !(cause instanceof DriveGrantError) || cause.outcome === 'unknown';
      if (unresolved) ready = false;
      error = unresolved
        ? 'Drive grant outcome unknown. Refresh grants before deciding on another action; do not blindly retry.'
        : cause instanceof Error ? cause.message : 'Drive grant request failed.';
    } finally { if (request.current()) busy = false; }
  }
</script>

{#if canManage}
  <section class="drive-grants" aria-label="Worker Drive grants">
    <header><h2>Drive grants</h2></header>
    <p class="hint">Grants allow access to this Workspace’s Drive. Drive tool enablement in the Worker’s Profile is separate.</p>
    {#if !workersReady}
      <p role="status">Waiting for the current Worker catalog…</p>
    {:else if workers.length === 0}
      <p>No Workers are available for a Drive grant.</p>
    {:else}
      <label class="worker-select">Worker
        <select value={selected} onchange={(event) => { selected = event.currentTarget.value; }}>
          <option value="">Select a Worker</option>
          {#each workers as worker (key(worker))}
            <option value={key(worker)}>{worker.display_name || worker.label} · {worker.runtime_id} / {worker.worker_id}</option>
          {/each}
        </select>
      </label>
      {#if selectedWorker}
        {#key identityScope}
          <details class="identity">
            <summary>Worker identity</summary>
            <dl>
              <dt>Runtime</dt><dd><code>{selectedWorker.runtime_id}</code></dd>
              <dt>Worker</dt><dd><code>{selectedWorker.worker_id}</code></dd>
            </dl>
          </details>
        {/key}
        <div class="grant-toolbar">
          <button type="button" disabled={loading || busy} onclick={() => refresh()}>Refresh grants</button>
          {#if ready && !unresolved && active.length === 0}
            <button type="button" disabled={busy} aria-expanded={showCreate} onclick={() => { showCreate = !showCreate; }}>Add Drive grant</button>
          {/if}
        </div>
        {#if loading}<p role="status">Loading all grant pages…</p>{/if}
        {#if error}<p class="error" role="alert">{error}</p>{/if}
        {#if notice}<p role="status">{notice}</p>{/if}
        {#if ready && active.length === 0}<p>No active Drive grants for this Worker.</p>{/if}
        {#if ready && active.length > 0}
          <p class="hint">Revoke the current grant before changing access.</p>
          <ul class="grant-list" aria-label="Active Drive grants">
            {#each active as grant (grant.grant_id)}
              <li>
                <span><strong>{grant.access === 'read_only' ? 'Read only' : 'Read and write'}</strong> <span class="hint">Grant {grant.grant_id}</span></span>
                <button class="revoke" type="button" disabled={busy || loading || unresolved} aria-label={`Revoke Drive grant ${grant.grant_id}`} onclick={() => mutate(grant)}>Revoke</button>
              </li>
            {/each}
          </ul>
        {/if}
        {#if showCreate && ready && !unresolved && active.length === 0}
          <form onsubmit={(event) => { event.preventDefault(); void mutate(); }}>
            <label>Access
              <select value={access} onchange={(event) => { const value = event.currentTarget.value; if (value === 'read_only' || value === 'read_write') access = value; }} disabled={busy}>
                <option value="read_only">Read only</option>
                <option value="read_write">Read and write</option>
              </select>
            </label>
            <button type="submit" disabled={busy || loading}>{busy ? 'Saving…' : 'Create Drive grant'}</button>
          </form>
        {/if}
      {/if}
    {/if}
  </section>
{/if}

<style>
  .drive-grants { display: grid; gap: var(--space-3); margin-top: var(--space-6); min-width: 0; }
  h2, p { margin: 0; }
  h2 { font-size: var(--font-size-body); line-height: var(--line-height-body); }
  .hint { color: var(--text-muted); }
  .worker-select, form label { display: grid; gap: var(--space-2); min-width: 0; }
  select { max-width: 100%; width: 100%; min-width: 0; }
  button, select { padding: var(--space-2) var(--space-3); border: 1px solid var(--line); border-radius: var(--radius-soft); background: var(--bg-raised); color: var(--text); font: inherit; }
  button { cursor: pointer; }
  button:hover:not(:disabled) { background: var(--interactive-hover); }
  button:focus-visible, select:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  button:disabled { opacity: 0.5; cursor: not-allowed; }
  .grant-toolbar, form { display: flex; flex-wrap: wrap; align-items: end; gap: var(--space-2); }
  form label { flex: 1 1 12rem; }
  .grant-list { list-style: none; margin: 0; padding: 0; }
  li { display: flex; justify-content: space-between; align-items: center; gap: var(--space-3); padding-block: var(--space-2); border-bottom: 1px solid var(--line); }
  li span { overflow-wrap: anywhere; }
  .identity summary { cursor: pointer; }
  .identity summary:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .identity dl { display: grid; grid-template-columns: auto minmax(0, 1fr); gap: var(--space-2); margin: var(--space-2) 0 0; }
  .identity dd { margin: 0; overflow-wrap: anywhere; }
  .error { overflow-wrap: anywhere; }
  .error, .revoke { color: var(--danger); }
</style>
