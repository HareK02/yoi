<script lang="ts">
  import { workspaceApiPath } from '$lib/workspace-api/http';
  import { workerConsoleHref } from '$lib/workspace-console/model';
  import { buildBrowserCreateWorkerRequest, defaultWorkerLaunchForm } from './worker-launch';
  import type {
    BrowserCreateWorkerResponse,
    BrowserExecutionWorkspaceCreateResponse,
    ListResponse,
    Worker,
    WorkerLaunchOptionsResponse,
  } from './types';

  const MAX_VISIBLE_WORKERS = 6;

  type Props = {
    currentPath?: string;
    workspaceId: string;
  };

  let { currentPath = '/', workspaceId }: Props = $props();

  function workerApiPath(path: string): string {
    return workspaceApiPath(workspaceId, path);
  }

  let loading = $state(true);
  let error = $state<string | null>(null);
  let workers = $state<Worker[]>([]);
  let placeholder = $state<string | null>(null);
  let options = $state<WorkerLaunchOptionsResponse | null>(null);
  let optionsError = $state<string | null>(null);
  let showNewWorker = $state(false);
  let submitting = $state(false);
  let submitError = $state<string | null>(null);
  let displayName = $state('Coding Worker');
  let runtimeId = $state('');
  let profile = $state('builtin:coder');
  let initialText = $state('');
  let executionWorkspaceAllocationId = $state('');
  let executionWorkspaceRepositoryId = $state('');
  let executionWorkspaceSelector = $state('HEAD');
  let relativeCwd = $state('');
  let creatingWorkspace = $state(false);

  $effect(() => {
    if (!workspaceId) {
      loading = false;
      workers = [];
      options = null;
      return;
    }

    const controller = new AbortController();
    void loadWorkers(controller.signal);
    void loadLaunchOptions(controller.signal);
    return () => controller.abort();
  });

  async function loadWorkers(signal?: AbortSignal) {
    loading = true;
    error = null;
    placeholder = null;
    try {
      const response = await fetch(workerApiPath('/workers'), { signal });
      if (response.status === 404) {
        workers = [];
        placeholder = 'Worker API is not integrated in this build yet.';
        return;
      }
      if (!response.ok) {
        throw new Error(`workers request failed (${response.status})`);
      }
      const payload = (await response.json()) as ListResponse<Worker>;
      workers = Array.isArray(payload.items) ? payload.items.slice(0, MAX_VISIBLE_WORKERS) : [];
      if (workers.length === 0) {
        placeholder = 'No workers reported by the current API.';
      }
    } catch (err) {
      if (err instanceof DOMException && err.name === 'AbortError') {
        return;
      }
      error = err instanceof Error ? err.message : 'workers request failed';
      workers = [];
    } finally {
      if (!signal?.aborted) {
        loading = false;
      }
    }
  }

  async function loadLaunchOptions(signal?: AbortSignal) {
    optionsError = null;
    try {
      const response = await fetch(workerApiPath('/workers/launch-options'), { signal });
      if (!response.ok) {
        throw new Error(`worker launch options failed (${response.status})`);
      }
      const payload = (await response.json()) as WorkerLaunchOptionsResponse;
      options = payload;
      const form = defaultWorkerLaunchForm(payload, {
        runtime_id: runtimeId,
        display_name: displayName,
        profile,
        initial_text: initialText,
        execution_workspace_allocation_id: executionWorkspaceAllocationId,
        execution_workspace_repository_id: executionWorkspaceRepositoryId,
        execution_workspace_selector: executionWorkspaceSelector,
        relative_cwd: relativeCwd,
      });
      runtimeId = form.runtime_id;
      displayName = form.display_name;
      profile = form.profile;
      executionWorkspaceAllocationId = form.execution_workspace_allocation_id;
      executionWorkspaceRepositoryId = form.execution_workspace_repository_id;
      executionWorkspaceSelector = form.execution_workspace_selector;
      relativeCwd = form.relative_cwd;
    } catch (err) {
      if (err instanceof DOMException && err.name === 'AbortError') {
        return;
      }
      optionsError = err instanceof Error ? err.message : 'worker launch options failed';
    }
  }

  async function createExecutionWorkspace() {
    if (!executionWorkspaceRepositoryId) {
      submitError = 'select a repository before creating an execution workspace';
      return;
    }
    creatingWorkspace = true;
    submitError = null;
    try {
      const response = await fetch(workerApiPath('/execution-workspaces'), {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          repository_id: executionWorkspaceRepositoryId,
          selector: executionWorkspaceSelector || null,
          policy: { dirty_state: 'clean_point_only', cleanup: 'manual_or_worker_stop' },
        }),
      });
      if (!response.ok) {
        throw new Error(await responseErrorMessage(response, 'execution workspace create failed'));
      }
      const payload = (await response.json()) as BrowserExecutionWorkspaceCreateResponse;
      const items = options?.execution_workspaces ?? [];
      options = options
        ? { ...options, execution_workspaces: [...items.filter((item) => item.allocation_id !== payload.item.allocation_id), payload.item] }
        : options;
      executionWorkspaceAllocationId = payload.item.allocation_id;
    } catch (err) {
      submitError = err instanceof Error ? err.message : 'execution workspace create failed';
    } finally {
      creatingWorkspace = false;
    }
  }

  async function createWorker() {
    if (!workspaceId) {
      submitError = 'workspace id is unavailable';
      return;
    }

    submitError = null;
    submitting = true;
    try {
      const response = await fetch(workerApiPath('/workers'), {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(buildBrowserCreateWorkerRequest({
          runtime_id: runtimeId,
          display_name: displayName,
          profile,
          initial_text: initialText,
          execution_workspace_allocation_id: executionWorkspaceAllocationId,
          execution_workspace_repository_id: executionWorkspaceRepositoryId,
          execution_workspace_selector: executionWorkspaceSelector,
          relative_cwd: relativeCwd,
        })),
      });
      if (!response.ok) {
        throw new Error(await responseErrorMessage(response, 'worker create failed'));
      }
      const payload = (await response.json()) as BrowserCreateWorkerResponse;
      await loadWorkers();
      window.location.href = payload.console_href;
    } catch (err) {
      submitError = err instanceof Error ? err.message : 'worker create failed';
    } finally {
      submitting = false;
    }
  }

  async function responseErrorMessage(response: Response, fallback: string): Promise<string> {
    try {
      const payload = (await response.json()) as { error?: { message?: string; code?: string } | string; message?: string };
      if (typeof payload.error === 'object' && payload.error?.message) {
        return `${payload.error.code ?? 'request_failed'}: ${payload.error.message}`;
      }
      if (payload.message) {
        const code = typeof payload.error === 'string' ? payload.error : 'request_failed';
        return `${code}: ${payload.message}`;
      }
    } catch {
      // fall through
    }
    return `${fallback} (${response.status})`;
  }
</script>

<section class="nav-section" aria-labelledby="workers-heading">
  <div class="section-heading-row">
    <h2 id="workers-heading">workers</h2>
    <button type="button" class="section-action" onclick={() => (showNewWorker = !showNewWorker)}>
      {showNewWorker ? 'Close' : 'New'}
    </button>
    {#if !loading && !error && workers.length > 0}
      <span class="section-count">{workers.length}</span>
    {/if}
  </div>

  {#if showNewWorker}
    <form class="worker-new-form" onsubmit={(event) => { event.preventDefault(); void createWorker(); }}>
      <label>
        <span>Display name</span>
        <input bind:value={displayName} required maxlength="80" autocomplete="off" />
      </label>
      <label>
        <span>Runtime</span>
        <select bind:value={runtimeId} required>
          {#if options?.runtimes.length}
            {#each options.runtimes as runtime}
              <option value={runtime.runtime_id} disabled={!runtime.can_spawn_worker}>
                {runtime.display_name} · {runtime.status}{runtime.built_in ? ' · embedded' : ''}
              </option>
            {/each}
          {:else}
            <option value="" disabled>No Runtime options</option>
          {/if}
        </select>
      </label>
      <label>
        <span>Profile</span>
        <select bind:value={profile} required>
          {#if options?.profiles.length}
            {#each options.profiles as candidate}
              <option value={candidate.id}>{candidate.label}</option>
            {/each}
          {:else}
            <option value="" disabled>No profile candidates</option>
          {/if}
        </select>
      </label>
      <fieldset class="worker-execution-workspace">
        <legend>Execution workspace</legend>
        <label>
          <span>Allocation</span>
          <select bind:value={executionWorkspaceAllocationId}>
            <option value="">No allocation selected</option>
            {#each options?.execution_workspaces ?? [] as workspace}
              <option value={workspace.allocation_id} disabled={workspace.status !== 'active'}>
                {workspace.repository_id} · {workspace.requested_selector ?? 'HEAD'} · {workspace.resolved_commit.slice(0, 12)} · {workspace.status}
              </option>
            {/each}
          </select>
        </label>
        <label>
          <span>Repository for new allocation</span>
          <select bind:value={executionWorkspaceRepositoryId}>
            {#if options?.repositories.length}
              {#each options.repositories as repository}
                <option value={repository.id}>{repository.display_name}</option>
              {/each}
            {:else}
              <option value="" disabled>No configured repositories</option>
            {/if}
          </select>
        </label>
        <label>
          <span>Selector</span>
          <input bind:value={executionWorkspaceSelector} autocomplete="off" placeholder="HEAD" />
        </label>
        <button type="button" disabled={creatingWorkspace || !executionWorkspaceRepositoryId} onclick={() => void createExecutionWorkspace()}>
          {creatingWorkspace ? 'Allocating…' : 'Create execution workspace'}
        </button>
        <label>
          <span>Relative cwd</span>
          <input bind:value={relativeCwd} autocomplete="off" placeholder="Optional path inside allocation" />
        </label>
      </fieldset>
      <label>
        <span>Initial text</span>
        <textarea bind:value={initialText} rows="3" placeholder="Optional first instruction"></textarea>
      </label>
      {#if optionsError}
        <p class="section-state error">{optionsError}</p>
      {/if}
      {#if submitError}
        <p class="section-state error">{submitError}</p>
      {/if}
      <button type="submit" disabled={submitting || !runtimeId || !profile || !executionWorkspaceAllocationId}>
        {submitting ? 'Starting…' : 'Start Coding Worker'}
      </button>
    </form>
  {/if}

  {#if loading}
    <p class="section-state">Checking workers…</p>
  {:else if error}
    <p class="section-state error">{error}</p>
  {:else if workers.length === 0}
    <p class="section-state">{placeholder ?? 'Workers will appear here when an API is connected.'}</p>
  {:else}
    <ul class="nav-list" aria-label="Workers">
      {#each workers as worker (`${worker.runtime_id}:${worker.worker_id}`)}
        {@const href = workerConsoleHref(worker, workspaceId)}
        <li>
          <a href={href} class="nav-item worker-nav-item" class:active={currentPath === href} aria-current={currentPath === href ? 'page' : undefined}>
            <span class="worker-title-row">
              <span class="item-title">{worker.label}</span>
              <span class="worker-task-title">-</span>
            </span>
            <span class="item-meta">
              {worker.role ? `${worker.role} · ` : ''}{worker.state} · {worker.status} · 🖥 {worker.host_id}
              {worker.execution_workspace ? ` · ws:${worker.execution_workspace.repository_id}@${worker.execution_workspace.resolved_commit.slice(0, 8)}` : ''}
            </span>
          </a>
        </li>
      {/each}
    </ul>
  {/if}
</section>
