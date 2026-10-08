import type {
  Event as ProtocolEvent,
  InFlightCompaction,
  SessionSnapshotEntry,
  WorkerStateSnapshot,
} from "#lib/generated/protocol.ts";

export type RunActivityStats = {
  /** Request counts follow persisted usage when the producer supplies it. */
  fromLog?: boolean;
  startedAtMs: number | null;
  requests: number;
  uploadTokens: number;
  outputTokens: number;
};

export function emptyRunActivityStats(): RunActivityStats {
  return {
    startedAtMs: null,
    requests: 0,
    uploadTokens: 0,
    outputTokens: 0,
  };
}

export function restoredRunActivity(entries: SessionSnapshotEntry[]): RunActivityStats {
  let stats = emptyRunActivityStats();
  for (const entry of entries) {
    if (entry.kind === "invoke") {
      stats = applyRunActivityEvent(stats, { event: "invoke_start", data: {
        kind: entry.trigger, timestamp_ms: entry.timestamp,
      } }, entry.timestamp);
    } else if (entry.kind === "usage") {
      stats = applyRunActivityEvent(stats, { event: "usage", data: {
        timestamp_ms: entry.timestamp,
        input_tokens: entry.input_tokens,
        cache_read_input_tokens: entry.cache_read_input_tokens,
        output_tokens: entry.output_tokens,
      } }, entry.timestamp);
    }
  }
  return stats;
}

export function applyRunActivityEvent(
  current: RunActivityStats,
  event: ProtocolEvent,
  observedAtMs: number,
): RunActivityStats {
  switch (event.event) {
    case "invoke_start":
      return {
        ...emptyRunActivityStats(),
        ...(event.data.timestamp_ms != null ? { fromLog: true } : {}),
        startedAtMs: event.data.timestamp_ms ?? observedAtMs,
      };
    case "snapshot": {
      const restored = restoredRunActivity(event.data.session?.entries ?? []);
      if (restored.fromLog) return restored;
      return event.data.state.state.kind === "busy" &&
          !(event.data.state.state.state.kind === "run" &&
            event.data.state.state.state.state === "paused")
        ? { ...emptyRunActivityStats(), startedAtMs: observedAtMs }
        : emptyRunActivityStats();
    }
    case "turn_start":
      if (current.fromLog) return current;
      return {
        ...current,
        startedAtMs: current.startedAtMs ?? observedAtMs,
        requests: current.requests + 1,
      };
    case "usage": {
      const input = event.data.input_tokens ?? 0;
      const cacheRead = event.data.cache_read_input_tokens ?? 0;
      const fromLog = event.data.timestamp_ms != null || current.fromLog;
      return {
        ...current,
        ...(fromLog ? { fromLog: true } : {}),
        requests: current.requests + (fromLog ? 1 : 0),
        startedAtMs: current.startedAtMs ?? (fromLog ? null : observedAtMs),
        uploadTokens: current.uploadTokens + Math.max(0, input - cacheRead),
        outputTokens: current.outputTokens + (event.data.output_tokens ?? 0),
      };
    }
    default:
      return current;
  }
}

export function formatRunElapsed(elapsedMs: number): string {
  const totalSeconds = Math.max(0, Math.floor(elapsedMs / 1_000));
  const hours = Math.floor(totalSeconds / 3_600);
  const minutes = Math.floor((totalSeconds % 3_600) / 60);
  const seconds = totalSeconds % 60;
  if (hours > 0) return `${hours}h ${minutes}m ${seconds}s`;
  if (minutes > 0) return `${minutes}m ${seconds}s`;
  return `${seconds}s`;
}

export function formatRunElapsedCompact(elapsedMs: number): string {
  return formatRunElapsed(elapsedMs).replaceAll(" ", "");
}

/** Match the TUI token abbreviation contract. */
export function formatRunTokens(tokens: number): string {
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(1)}M`;
  if (tokens >= 1_000) return `${(tokens / 1_000).toFixed(1)}k`;
  return String(tokens);
}

export type CompactionStatusPresentation = {
  progress: InFlightCompaction;
  label: string;
};

export function resolveCompactionStatusPresentation(
  compaction: InFlightCompaction | null,
  workerState: WorkerStateSnapshot | null,
): CompactionStatusPresentation | null {
  const progress = visibleCompactionProgress(compaction, workerState);
  return progress
    ? { progress, label: `Compacting · ${progress.phase}` }
    : null;
}

export function visibleCompactionProgress(
  compaction: InFlightCompaction | null,
  workerState: WorkerStateSnapshot | null,
): InFlightCompaction | null {
  if (!compaction || workerState?.state.kind !== "busy") {
    return null;
  }

  const busyState = workerState.state.state;
  const triggerMatches = compaction.trigger === "manual"
    ? busyState.kind === "maintenance" &&
      busyState.state === "compacting"
    : busyState.kind === "run";

  return triggerMatches ? compaction : null;
}
