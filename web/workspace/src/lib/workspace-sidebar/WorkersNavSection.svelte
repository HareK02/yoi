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

<style>
  .nav-section {
    display: grid;
    gap: 10px;
  }

  .section-heading-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }

  h2 {
    margin: 0;
    color: #94a3b8;
    font-size: 0.72rem;
    font-weight: 800;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }

  .section-count {
    border: 1px solid rgba(148, 163, 184, 0.22);
    border-radius: 999px;
    color: #cbd5e1;
    font-size: 0.72rem;
    line-height: 1;
    padding: 4px 8px;
  }

  .nav-list {
    display: grid;
    gap: 6px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .nav-item {
    display: grid;
    gap: 3px;
    border: 1px solid rgba(148, 163, 184, 0.18);
    border-radius: 14px;
    background: rgba(15, 23, 42, 0.64);
    padding: 10px 12px;
    min-width: 0;
  }

  .item-title,
  .item-meta {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .item-title {
    color: #e2e8f0;
    font-weight: 700;
  }

  .item-meta,
  .section-state {
    color: #94a3b8;
    font-size: 0.78rem;
  }

  .section-state {
    margin: 0;
    border: 1px dashed rgba(148, 163, 184, 0.2);
    border-radius: 14px;
    padding: 10px 12px;
    line-height: 1.45;
  }

  .section-state.error {
    border-color: rgba(248, 113, 113, 0.36);
    color: #fecaca;
  }
</style>
