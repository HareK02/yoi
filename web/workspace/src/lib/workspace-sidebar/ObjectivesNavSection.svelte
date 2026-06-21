<script lang="ts">
  import type { ObjectiveListResponse, ObjectiveSummary } from './types';

  const MAX_VISIBLE_OBJECTIVES = 6;

  let loading = $state(true);
  let error = $state<string | null>(null);
  let objectives = $state<ObjectiveSummary[]>([]);
  let invalidRecordCount = $state(0);

  $effect(() => {
    const controller = new AbortController();
    void loadObjectives(controller.signal);
    return () => controller.abort();
  });

  async function loadObjectives(signal: AbortSignal) {
    loading = true;
    error = null;
    try {
      const response = await fetch('/api/objectives', { signal });
      if (!response.ok) {
        throw new Error(`objectives request failed (${response.status})`);
      }
      const payload = (await response.json()) as ObjectiveListResponse;
      objectives = Array.isArray(payload.items) ? payload.items.slice(0, MAX_VISIBLE_OBJECTIVES) : [];
      invalidRecordCount = Array.isArray(payload.invalid_records) ? payload.invalid_records.length : 0;
    } catch (err) {
      if (err instanceof DOMException && err.name === 'AbortError') {
        return;
      }
      error = err instanceof Error ? err.message : 'objectives request failed';
      objectives = [];
      invalidRecordCount = 0;
    } finally {
      if (!signal.aborted) {
        loading = false;
      }
    }
  }
</script>

<section class="nav-section" aria-labelledby="objectives-heading">
  <div class="section-heading-row">
    <h2 id="objectives-heading">objectives</h2>
    {#if !loading && !error}
      <span class="section-count">{objectives.length}</span>
    {/if}
  </div>

  {#if loading}
    <p class="section-state">Loading objectives…</p>
  {:else if error}
    <p class="section-state error">{error}</p>
  {:else if objectives.length === 0}
    <p class="section-state">No objectives found.</p>
  {:else}
    <ul class="nav-list" aria-label="Objectives">
      {#each objectives as objective (objective.id)}
        <li class="nav-item">
          <span class="item-title">{objective.title}</span>
          <span class="item-meta">{objective.state}</span>
        </li>
      {/each}
    </ul>
  {/if}

  {#if invalidRecordCount > 0}
    <p class="section-note">{invalidRecordCount} invalid objective record(s) hidden.</p>
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
  .section-state,
  .section-note {
    color: #94a3b8;
    font-size: 0.78rem;
  }

  .section-state,
  .section-note {
    margin: 0;
    line-height: 1.45;
  }

  .section-state {
    border: 1px dashed rgba(148, 163, 184, 0.2);
    border-radius: 14px;
    padding: 10px 12px;
  }

  .section-state.error {
    border-color: rgba(248, 113, 113, 0.36);
    color: #fecaca;
  }
</style>
