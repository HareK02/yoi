<script lang="ts">
  import type { InFlightCompaction } from "$lib/generated/protocol.ts";
  import Spinner from "./Spinner.svelte";
  import { formatRunElapsed, formatRunTokens } from "./run-status";

  type Props = {
    startedAtMs: number | null;
    requests: number;
    uploadTokens: number;
    outputTokens: number;
    compaction?: InFlightCompaction | null;
  };

  let { startedAtMs, requests, uploadTokens, outputTokens, compaction = null }: Props =
    $props();
  let nowMs = $state(Date.now());
  const clockStartedAtMs = $derived(compaction?.started_at_ms ?? startedAtMs);

  $effect(() => {
    clockStartedAtMs;
    nowMs = Date.now();
    const timer = window.setInterval(() => {
      nowMs = Date.now();
    }, 1_000);
    return () => window.clearInterval(timer);
  });

  const elapsed = $derived(
    formatRunElapsed(nowMs - (clockStartedAtMs ?? nowMs)),
  );
  const requestLabel = $derived(requests === 1 ? "req" : "reqs");
</script>

<div class="worker-run-status" role="status" aria-live="off">
  <Spinner />
  {#if compaction}
    <span>Compacting · {compaction.phase}</span>
    <span aria-hidden="true">|</span>
  {/if}
  <span>{elapsed}</span>
  <span aria-hidden="true">・</span>
  <span>{requests} {requestLabel}</span>
  <span aria-hidden="true">|</span>
  <span>↑{formatRunTokens(uploadTokens)}/↓{formatRunTokens(outputTokens)}</span>
</div>

<style>
  .worker-run-status {
    display: flex;
    align-items: center;
    gap: 0.42rem;
    min-height: 1.35rem;
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: 0.74rem;
    font-variant-numeric: tabular-nums;
  }
</style>
