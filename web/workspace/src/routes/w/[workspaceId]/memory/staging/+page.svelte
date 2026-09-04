<script lang="ts">
  import type { MemoryStagingEntry, MemoryStagingRecord } from '$lib/generated/memory-api';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();

  const entries = $derived(data.staging.data?.items ?? []);

  function kindLabel(kind: string): string {
    return kind.replaceAll('_', ' ');
  }

  function sourceLabel(record: MemoryStagingRecord): string {
    const source = record.source;
    return `${source.segment_id}:${source.range[0]}-${source.range[1]}`;
  }

  function evidenceCount(entry: MemoryStagingEntry): number {
    return entry.record.evidence?.length ?? 0;
  }

  function sourceRefCount(entry: MemoryStagingEntry): number {
    return entry.record.source_refs?.length ?? 0;
  }
</script>

<svelte:head>
  <title>Memory Staging · Yoi Workspace</title>
  <meta name="description" content="Workspace Memory Staging" />
</svelte:head>

<section class="card memory-staging-card">
  <div class="detail-heading">
    <div>
      <p class="eyebrow">Workspace memory</p>
      <h2>Memory Staging</h2>
    </div>
    {#if data.staging.data}
      <span>{data.staging.data.returned_count} / {data.staging.data.total_valid_count} staged</span>
    {/if}
  </div>

  <p class="section-note">
    Pending Memory extraction candidates staged for consolidation. This view is read-only and uses the Workspace Server memory authority.
  </p>

  {#if data.staging.data}
    <div class="staging-summary-grid" aria-label="Memory staging summary">
      <div>
        <span>Valid records</span>
        <strong>{data.staging.data.total_valid_count}</strong>
      </div>
      <div>
        <span>Invalid records</span>
        <strong>{data.staging.data.invalid_count}</strong>
      </div>
      <div>
        <span>Order</span>
        <strong>{data.staging.data.order}</strong>
      </div>
      <div>
        <span>Authority</span>
        <strong>{data.staging.data.record_authority}</strong>
      </div>
    </div>

    {#if data.staging.data.truncated}
      <p class="section-note">Showing first {data.staging.data.limit} staged record(s).</p>
    {/if}

    {#each data.staging.data.diagnostics as diagnostic (diagnostic.code)}
      <p class:error={diagnostic.severity === 'error'} class="section-note">
        {diagnostic.message}
      </p>
    {/each}

    {#if entries.length === 0}
      <p>No Memory Staging records are present.</p>
    {:else}
      <div class="staging-list">
        {#each entries as entry (entry.id)}
          <article class="staging-entry">
            <header>
              <div>
                <span class="kind-pill">{kindLabel(entry.record.kind)}</span>
                <h3>{entry.record.claim}</h3>
              </div>
              <code>{entry.id}</code>
            </header>

            <p>{entry.record.why_useful}</p>

            {#if entry.record.staleness}
              <p class="staleness">Staleness: {entry.record.staleness}</p>
            {/if}

            <dl class="staging-meta">
              <div>
                <dt>Extract run</dt>
                <dd>{entry.record.extract_run_id}</dd>
              </div>
              <div>
                <dt>Source</dt>
                <dd>{sourceLabel(entry.record)}</dd>
              </div>
              <div>
                <dt>Evidence</dt>
                <dd>{evidenceCount(entry)}</dd>
              </div>
              <div>
                <dt>Source refs</dt>
                <dd>{sourceRefCount(entry)}</dd>
              </div>
              <div>
                <dt>Bytes</dt>
                <dd>{entry.byte_len}</dd>
              </div>
            </dl>

            {#if entry.record.evidence && entry.record.evidence.length > 0}
              <details>
                <summary>Evidence</summary>
                <ul class="evidence-list">
                  {#each entry.record.evidence as evidence (evidence.id)}
                    <li>
                      <strong>{evidence.id}</strong>
                      <span>{evidence.kind}</span>
                      {#if evidence.summary}
                        <p>{evidence.summary}</p>
                      {/if}
                      {#if evidence.excerpt}
                        <blockquote>{evidence.excerpt}</blockquote>
                      {/if}
                    </li>
                  {/each}
                </ul>
              </details>
            {/if}
          </article>
        {/each}
      </div>
    {/if}
  {:else if data.staging.error}
    <p class="error">{data.staging.error}</p>
  {:else}
    <p>Waiting for <code>/api/w/{data.workspaceId}/memory/staging</code>…</p>
  {/if}
</section>

<style>
  .memory-staging-card {
    overflow: hidden;
  }

  .staging-summary-grid {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 0.75rem;
    margin: 1rem 0;
  }

  .staging-summary-grid div {
    border: 1px solid var(--line);
    border-radius: 0.75rem;
    background: var(--bg-raised);
    padding: 0.8rem;
  }

  .staging-summary-grid span,
  .staging-meta dt {
    color: var(--text-muted);
    display: block;
    font-size: 0.76rem;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  .staging-summary-grid strong {
    color: var(--text);
    display: block;
    font-size: 0.95rem;
    margin-top: 0.25rem;
    overflow-wrap: anywhere;
  }

  .staging-list {
    display: grid;
    gap: 0.9rem;
    margin-top: 1rem;
  }

  .staging-entry {
    border: 1px solid var(--line);
    border-radius: 0.9rem;
    background: var(--bg-raised);
    padding: 1rem;
  }

  .staging-entry header {
    align-items: flex-start;
    display: flex;
    gap: 1rem;
    justify-content: space-between;
  }

  .staging-entry h3 {
    color: var(--text-strong);
    font-size: 1rem;
    margin: 0.35rem 0 0;
  }

  .staging-entry p {
    color: var(--text);
    margin: 0.75rem 0 0;
  }

  .staging-entry code,
  .staging-meta dd {
    overflow-wrap: anywhere;
  }

  .kind-pill {
    border: 1px solid var(--line);
    border-radius: 999px;
    color: var(--accent);
    display: inline-flex;
    font-size: 0.72rem;
    font-weight: 700;
    letter-spacing: 0.04em;
    padding: 0.18rem 0.55rem;
    text-transform: uppercase;
  }

  .staleness {
    color: var(--warning);
  }

  .staging-meta {
    display: grid;
    grid-template-columns: repeat(5, minmax(0, 1fr));
    gap: 0.75rem;
    margin: 1rem 0 0;
  }

  .staging-meta div {
    min-width: 0;
  }

  .staging-meta dd {
    color: var(--text);
    margin: 0.25rem 0 0;
  }

  details {
    border-top: 1px solid var(--line);
    margin-top: 1rem;
    padding-top: 0.75rem;
  }

  summary {
    color: var(--text-muted);
    cursor: pointer;
    font-weight: 700;
  }

  .evidence-list {
    display: grid;
    gap: 0.75rem;
    list-style: none;
    margin: 0.75rem 0 0;
    padding: 0;
  }

  .evidence-list li {
    border: 1px solid var(--line);
    border-radius: 0.7rem;
    background: var(--bg-subtle);
    padding: 0.75rem;
  }

  .evidence-list span {
    color: var(--text-muted);
    margin-left: 0.5rem;
  }

  blockquote {
    border-left: 3px solid var(--line-strong);
    color: var(--text-muted);
    margin: 0.75rem 0 0;
    padding-left: 0.75rem;
  }

  @media (max-width: 900px) {
    .staging-summary-grid,
    .staging-meta {
      grid-template-columns: 1fr;
    }

    .staging-entry header {
      display: grid;
    }
  }
</style>
