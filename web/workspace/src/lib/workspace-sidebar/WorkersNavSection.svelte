<script lang="ts">
  import { workerConsoleHref } from '$lib/workspace-console/model';
  import type {
    BrowserCreateWorkerResponse,
    ListResponse,
    Worker,
    WorkerLaunchOptionsResponse,
  } from './types';

  const MAX_VISIBLE_WORKERS = 6;

  type Props = {
    currentPath?: string;
  };

  let { currentPath = '/' }: Props = $props();

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

  $effect(() => {
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
      const response = await fetch('/api/workers', { signal });
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
      const response = await fetch('/api/workers/launch-options', { signal });
      if (!response.ok) {
        throw new Error(`worker launch options failed (${response.status})`);
      }
      const payload = (await response.json()) as WorkerLaunchOptionsResponse;
      options = payload;
      const preferredRuntime = payload.runtimes.find((runtime) => runtime.can_spawn_worker && runtime.status === 'active')
        ?? payload.runtimes.find((runtime) => runtime.can_spawn_worker)
        ?? payload.runtimes[0];
      if (preferredRuntime && !runtimeId) {
        runtimeId = preferredRuntime.runtime_id;
      }
      const preferredProfile = payload.profiles.find((candidate) => candidate.id === 'builtin:coder') ?? payload.profiles[0];
      if (preferredProfile && !payload.profiles.some((candidate) => candidate.id === profile)) {
        profile = preferredProfile.id;
      }
    } catch (err) {
      if (err instanceof DOMException && err.name === 'AbortError') {
        return;
      }
      optionsError = err instanceof Error ? err.message : 'worker launch options failed';
    }
  }

  async function createWorker() {
    submitError = null;
    submitting = true;
    try {
      const response = await fetch('/api/workers', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          runtime_id: runtimeId,
          display_name: displayName,
          profile,
          initial_text: initialText,
        }),
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
      <button type="submit" disabled={submitting || !runtimeId || !profile}>
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
        {@const href = workerConsoleHref(worker)}
        <li>
          <a href={href} class="nav-item worker-nav-item" class:active={currentPath === href} aria-current={currentPath === href ? 'page' : undefined}>
            <span class="worker-title-row">
              <span class="item-title">{worker.label}</span>
              <span class="worker-task-title">-</span>
            </span>
            <span class="item-meta">
              {worker.role ? `${worker.role} · ` : ''}{worker.state} · {worker.status} · 🖥 {worker.host_id}
            </span>
          </a>
        </li>
      {/each}
    </ul>
  {/if}
</section>
