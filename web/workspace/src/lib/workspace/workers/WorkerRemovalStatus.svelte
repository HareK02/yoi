<script lang="ts">
  import type { WorkerRemovalObservation } from '#lib/generated/worker-launch-api.ts';
  import { removalStateLabel, removalPhaseLabels, removalCauseLabels } from './worker-removal.ts';

  let { removal, compact = false }: { removal: WorkerRemovalObservation; compact?: boolean } = $props();
</script>

{#if compact}
  <span class="removal-compact">
    {removalStateLabel(removal)} · {removalPhaseLabels[removal.phase]}
    {#if removal.cause} · {removalCauseLabels[removal.cause]}{/if}
  </span>
{:else}
  <div class="removal-status" role="status">
    <strong>{removalStateLabel(removal)}</strong>
    <span>{removalPhaseLabels[removal.phase]}</span>
    {#if removal.cause}<span>{removalCauseLabels[removal.cause]}</span>{/if}
    <span>Worker retained, not deleted.</span>
    <details>
      <summary>Removal operation</summary>
      <code>{removal.operation_id}</code>
    </details>
  </div>
{/if}

<style>
  .removal-status {
    display: grid;
    gap: var(--space-1);
    min-width: 16rem;
    max-width: 24rem;
    margin-top: var(--space-2);
    font-size: var(--font-size-body);
    line-height: var(--line-height-body);
    white-space: normal;
    overflow-wrap: anywhere;
  }
  .removal-status strong { color: var(--text-strong); }
  .removal-status code { font-family: var(--font-mono); }
  .removal-status summary { cursor: pointer; }
  .removal-status summary:focus-visible { outline: 1px solid var(--accent); }
  .removal-compact {
    display: block;
    grid-column: 2;
    min-width: 0;
    color: var(--text-muted);
    font-size: var(--font-size-compact);
    line-height: var(--line-height-compact);
    white-space: normal;
    overflow-wrap: anywhere;
  }
</style>
