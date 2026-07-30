<script lang="ts">
  import { workspaceApiPath } from '$lib/workspace/api/http';
  import { workerConsoleHref } from '$lib/workspace/console/model';
  import { canShowWorkerInSidebar } from './workers';
  import type { ListResponse, Worker } from './types';

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

  $effect(() => {
    if (!workspaceId) {
      loading = false;
      workers = [];
      return;
    }

    const controller = new AbortController();
    void loadWorkers(controller.signal);
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
      workers = Array.isArray(payload.items)
        ? payload.items.filter(canShowWorkerInSidebar).slice(0, MAX_VISIBLE_WORKERS)
        : [];
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
</script>

<section class="nav-section" aria-labelledby="workers-heading">
  <div class="section-heading-row">
    <h2 id="workers-heading">
      <a
        class="section-heading-link"
        class:active={currentPath === `/w/${workspaceId}/workers`}
        href={`/w/${workspaceId}/workers`}
        aria-current={currentPath === `/w/${workspaceId}/workers` ? 'page' : undefined}
      >workers</a>
    </h2>
    <a
      class="section-action"
      class:active={currentPath === `/w/${workspaceId}/workers/new`}
      href={`/w/${workspaceId}/workers/new`}
      aria-current={currentPath === `/w/${workspaceId}/workers/new` ? 'page' : undefined}
    >
      New
    </a>
    {#if !loading && !error && workers.length > 0}
      <span class="section-count">{workers.length}</span>
    {/if}
  </div>

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
              <span class="item-title">{worker.display_name || worker.label}</span>
              <span class="worker-task-title">-</span>
            </span>
            <span class="item-meta">
              worker {worker.worker_id} · {worker.profile ? `${worker.profile} · ` : ''}{worker.state} · 🖥 {worker.host_id}
              {worker.working_directory ? ` · wd:${worker.working_directory.repository_id}@${worker.working_directory.resolved_commit.slice(0, 8)}` : ''}
            </span>
          </a>
        </li>
      {/each}
    </ul>
  {/if}
</section>
