<script lang="ts">
  import type { ListResponse, Worker } from './types';

  const MAX_VISIBLE_WORKERS = 6;

  let loading = $state(true);
  let error = $state<string | null>(null);
  let workers = $state<Worker[]>([]);
  let placeholder = $state<string | null>(null);

  $effect(() => {
    const controller = new AbortController();
    void loadWorkers(controller.signal);
    return () => controller.abort();
  });

  async function loadWorkers(signal: AbortSignal) {
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
      if (!signal.aborted) {
        loading = false;
      }
    }
  }
</script>

<section class="nav-section" aria-labelledby="workers-heading">
  <div class="section-heading-row">
    <h2 id="workers-heading">workers</h2>
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
      {#each workers as worker (worker.worker_id)}
        <li class="nav-item">
          <span class="item-title">{worker.label}</span>
          <span class="item-meta">
            {worker.state} · {worker.status}{worker.role ? ` · ${worker.role}` : ''}
          </span>
        </li>
      {/each}
    </ul>
  {/if}
</section>
