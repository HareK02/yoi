import type { Event as ProtocolEvent } from "$lib/generated/protocol";

export type RunActivityStats = {
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

export function applyRunActivityEvent(
  current: RunActivityStats,
  event: ProtocolEvent,
  observedAtMs: number,
): RunActivityStats {
  switch (event.event) {
    case "invoke_start":
      return { ...emptyRunActivityStats(), startedAtMs: observedAtMs };
    case "snapshot":
      return event.data.status === "running"
        ? { ...emptyRunActivityStats(), startedAtMs: observedAtMs }
        : emptyRunActivityStats();
    case "turn_start":
      return {
        ...current,
        startedAtMs: current.startedAtMs ?? observedAtMs,
        requests: current.requests + 1,
      };
    case "usage": {
      const input = event.data.input_tokens ?? 0;
      const cacheRead = event.data.cache_read_input_tokens ?? 0;
      return {
        ...current,
        startedAtMs: current.startedAtMs ?? observedAtMs,
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
