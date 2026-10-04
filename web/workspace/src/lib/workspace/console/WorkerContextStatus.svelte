<script lang="ts">
  import type { Snippet } from "svelte";
  import type { ConsoleWorkerMetadata } from "./model";
  import {
    formatContextSummary,
    formatModelSummary,
  } from "./worker-metadata";

  type Props = {
    metadata: ConsoleWorkerMetadata | null;
    controls?: Snippet;
    detailsOpen?: boolean;
    onToggleDetails?: () => void;
  };

  let { metadata, controls, detailsOpen = false, onToggleDetails }: Props = $props();
  const modelSummary = $derived(formatModelSummary(metadata));
  const contextSummary = $derived(formatContextSummary(metadata));
</script>

<div class="worker-context-status" aria-label="Worker model and context">
  <span class="worker-model" title={modelSummary}>{modelSummary}</span>
  <span class="separator" aria-hidden="true">·</span>
  <span class="worker-context" title={contextSummary}>{contextSummary}</span>
  {#if controls || onToggleDetails}
    <div class="worker-context-actions">
      {@render controls?.()}
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

  .worker-context-actions {
    display: flex;
    flex: 0 0 auto;
    align-items: center;
    gap: var(--space-2);
    margin-left: auto;
  }

  .details-button {
    display: inline-flex;
    flex: 0 0 auto;
    align-items: center;
    justify-content: center;
    width: var(--space-5);
    height: var(--space-5);
    padding: 0;
    border: 0;
    border-radius: var(--space-1);
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
  }

  .details-button:hover,
  .details-button[aria-expanded="true"] {
    background: var(--bg-subtle);
    color: var(--text-strong);
  }

  .details-button:focus-visible {
    color: var(--text-strong);
    outline: 1px solid currentColor;
    outline-offset: -2px;
  }

  .details-button svg {
    width: 16px;
    height: 16px;
    fill: currentColor;
  }
</style>
