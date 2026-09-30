<script lang="ts">
  import DocumentMarkdown from '$lib/workspace/markdown/DocumentMarkdown.svelte';
  import { formatDate } from '$lib/workspace/api/http';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
</script>

<svelte:head>
  <title>Memory · Yoi Workspace</title>
  <meta name="description" content="Workspace Memory document" />
</svelte:head>

<section class="memory-document-page" aria-label="Memory document">
  {#if data.memory.data}
    <div class="memory-document-meta">
      <p>
        <span>Read-only</span>
        <span aria-hidden="true">·</span>
        <span>Updated <time datetime={data.memory.data.updated_at}>{formatDate(data.memory.data.updated_at)}</time></span>
      </p>
      <details>
        <summary>Document details</summary>
        <dl>
          <div>
            <dt>Created</dt>
            <dd><time datetime={data.memory.data.created_at}>{formatDate(data.memory.data.created_at)}</time></dd>
          </div>
          <div>
            <dt>Size</dt>
            <dd>{data.memory.data.bytes} bytes</dd>
          </div>
          <div>
            <dt>Source</dt>
            <dd><code>{data.memory.data.record_source}</code></dd>
          </div>
        </dl>
      </details>
    </div>

    {#if data.memory.data.body_md.trim().length === 0}
      <div class="memory-document-state" role="status">
        <strong>Memory document is empty.</strong>
        <p>Durable Workspace context will appear here when it is available.</p>
      </div>
    {:else}
      <article class="memory-document-content" aria-label="Memory document content">
        <DocumentMarkdown text={data.memory.data.body_md} />
      </article>
    {/if}
  {:else if data.memory.error}
    <div class="memory-document-state memory-document-error">
      <p role="alert"><strong>Memory document unavailable.</strong> {data.memory.error}</p>
    </div>
  {:else}
    <div class="memory-document-state" role="status">
      <p>Memory document data is unavailable.</p>
    </div>
  {/if}
</section>

<style>
  .memory-document-page {
    display: grid;
    flex: 0 0 auto;
    gap: var(--space-5);
    width: 100%;
    min-width: 0;
    max-width: 78rem;
    margin-inline: auto;
  }

  .memory-document-meta {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--space-4);
    color: var(--text-muted);
    font-size: var(--font-size-compact);
    line-height: var(--line-height-compact);
  }

  .memory-document-meta > p {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    margin: 0;
  }

  .memory-document-meta details {
    flex: 0 0 auto;
  }

  .memory-document-meta summary {
    color: var(--text-muted);
    cursor: pointer;
  }

  .memory-document-meta dl {
    display: grid;
    gap: var(--space-2);
    min-width: min(24rem, calc(100vw - (2 * var(--space-4))));
    margin: var(--space-3) 0 0;
    padding: var(--space-3) 0 0;
    border-top: 1px solid var(--line);
  }

  .memory-document-meta dl > div {
    display: grid;
    grid-template-columns: 5rem minmax(0, 1fr);
    gap: var(--space-3);
  }

  .memory-document-meta dt {
    color: var(--text-faint);
  }

  .memory-document-meta dd {
    min-width: 0;
    margin: 0;
    color: var(--text);
    overflow-wrap: anywhere;
  }

  .memory-document-content {
    min-width: 0;
  }

  .memory-document-state {
    padding-block: var(--space-5);
    color: var(--text-muted);
  }

  .memory-document-state strong {
    color: var(--text-strong);
  }

  .memory-document-state p {
    margin: var(--space-1) 0 0;
  }

  .memory-document-state > p:first-child {
    margin-top: 0;
  }

  .memory-document-error,
  .memory-document-error strong {
    color: var(--danger);
  }

  @media (max-width: 600px) {
    .memory-document-meta {
      display: grid;
      gap: var(--space-2);
    }

    .memory-document-meta details {
      width: 100%;
    }

    .memory-document-meta dl {
      min-width: 0;
    }
  }
</style>
