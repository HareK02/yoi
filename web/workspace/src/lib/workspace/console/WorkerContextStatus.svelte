<script lang="ts">
  import type { ConsoleWorkerMetadata } from "./model";
  import {
    formatContextSummary,
    formatModelSummary,
  } from "./worker-metadata";

  type Props = {
    metadata: ConsoleWorkerMetadata | null;
    detailsOpen?: boolean;
    onToggleDetails?: () => void;
  };

  let { metadata, detailsOpen = false, onToggleDetails }: Props = $props();
  const modelSummary = $derived(formatModelSummary(metadata));
  const contextSummary = $derived(formatContextSummary(metadata));
</script>

<div class="worker-context-status" aria-label="Worker model and context">
  <span class="worker-model" title={modelSummary}>{modelSummary}</span>
  <span class="separator" aria-hidden="true">|</span>
  <span class="worker-context" title={contextSummary}>{contextSummary}</span>
  {#if onToggleDetails}
    <button
      type="button"
      class="details-button"
      aria-label="Details"
      aria-expanded={detailsOpen}
      onclick={onToggleDetails}
    >
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <circle cx="5" cy="12" r="2" />
        <circle cx="12" cy="12" r="2" />
        <circle cx="19" cy="12" r="2" />
      </svg>
    </button>
  {/if}
</div>

<style>
  .worker-context-status {
    display: flex;
    flex: 0 0 auto;
    flex-wrap: nowrap;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    padding-inline: var(--space-3);
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

  .details-button {
    display: inline-flex;
    flex: 0 0 auto;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: var(--line-height-compact);
    margin-left: auto;
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
  }

  .details-button:hover,
  .details-button[aria-expanded="true"],
  .details-button:focus-visible {
    color: var(--text);
  }

  .details-button:focus-visible {
    outline: 1px solid currentColor;
    outline-offset: -1px;
  }

  .details-button svg {
    width: 16px;
    height: 16px;
    fill: currentColor;
  }
</style>
