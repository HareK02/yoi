<script lang="ts">
  import { formatDate } from '$lib/workspace/api/http';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();

  const lineCount = $derived(data.memory.data?.body_md.split('\n').length ?? 0);
</script>

<svelte:head>
  <title>Memory Document · Yoi Workspace</title>
  <meta name="description" content="Workspace Memory Document" />
</svelte:head>

<section class="card memory-document-card">
  <div class="detail-heading">
    <div>
      <p class="eyebrow">Workspace memory</p>
      <h2>Memory Document</h2>
    </div>
    {#if data.memory.data}
      <span>{data.memory.data.bytes} bytes</span>
    {/if}
  </div>

  <p class="section-note">
    Durable workspace Memory as a single Markdown document. This view is read-only.
  </p>

  {#if data.memory.data}
    <div class="memory-document-summary" aria-label="Memory document summary">
      <div>
        <span>Updated</span>
        <strong>{formatDate(data.memory.data.updated_at)}</strong>
      </div>
      <div>
        <span>Created</span>
        <strong>{formatDate(data.memory.data.created_at)}</strong>
      </div>
      <div>
        <span>Lines</span>
        <strong>{lineCount}</strong>
      </div>
      <div>
        <span>Source</span>
        <strong>{data.memory.data.record_source}</strong>
      </div>
    </div>

    {#if data.memory.data.body_md.trim().length === 0}
      <p>No Memory document content is present.</p>
    {:else}
      <pre class="memory-document-body">{data.memory.data.body_md}</pre>
    {/if}
  {:else if data.memory.error}
    <p class="error">{data.memory.error}</p>
  {:else}
    <p>Waiting for <code>/api/w/{data.workspaceId}/memory</code>…</p>
  {/if}
</section>

<style>
  .memory-document-card {
    overflow: hidden;
  }

  .memory-document-summary {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 0.75rem;
    margin: 1rem 0;
  }

  .memory-document-summary div {
    border: 1px solid var(--line);
    border-radius: 0.75rem;
    background: var(--bg-raised);
    padding: 0.8rem;
  }

  .memory-document-summary span {
    color: var(--text-muted);
    display: block;
    font-size: 0.76rem;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  .memory-document-summary strong {
    color: var(--text);
    display: block;
    font-size: 0.95rem;
    margin-top: 0.25rem;
    overflow-wrap: anywhere;
  }

  .memory-document-body {
    background: var(--bg-raised);
    border: 1px solid var(--line);
    border-radius: 0.9rem;
    color: var(--text);
    font-family: var(--font-mono);
    font-size: 0.88rem;
    line-height: 1.6;
    margin: 1rem 0 0;
    overflow: auto;
    padding: 1rem;
    white-space: pre-wrap;
  }

  @media (max-width: 900px) {
    .memory-document-summary {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }

  @media (max-width: 640px) {
    .memory-document-summary {
      grid-template-columns: 1fr;
    }
  }
</style>
