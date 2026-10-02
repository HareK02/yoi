<script lang="ts">
  import type { ConsoleWorkerMetadata } from "./model";
  import {
    formatContextSummary,
    formatModelSummary,
  } from "./worker-metadata";

  type Props = {
    metadata: ConsoleWorkerMetadata | null;
  };

  let { metadata }: Props = $props();
  const modelSummary = $derived(formatModelSummary(metadata));
  const contextSummary = $derived(formatContextSummary(metadata));
</script>

<div class="worker-context-status" aria-label="Worker model and context">
  <span class="worker-model" title={modelSummary}>{modelSummary}</span>
  <span class="separator" aria-hidden="true">|</span>
  <span class="worker-context" title={contextSummary}>{contextSummary}</span>
</div>

<style>
  .worker-context-status {
    display: flex;
    min-width: 0;
    align-items: center;
    gap: var(--space-2);
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: var(--font-size-compact);
    line-height: var(--line-height-compact);
    font-variant-numeric: tabular-nums;
  }

  .worker-model,
  .worker-context {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .worker-model {
    color: var(--text);
  }

  .separator {
    flex: 0 0 auto;
    color: var(--line-strong);
  }

  @media (max-width: 640px) {
    .worker-context-status {
      width: 100%;
      flex-wrap: wrap;
      row-gap: 0;
    }

    .separator {
      display: none;
    }

    .worker-model,
    .worker-context {
      flex: 1 1 100%;
    }
  }
</style>
