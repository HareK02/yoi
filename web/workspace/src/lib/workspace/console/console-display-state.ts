export type ConsoleDisplaySource = "live" | "retained";

export type ConsoleDisplayState =
  | { kind: "loading"; stage: "session" | "snapshot" }
  | { kind: "ready"; source: ConsoleDisplaySource }
  | { kind: "unavailable"; reason: string }
  | { kind: "failed"; reason: string }
  | {
    kind: "stale";
    source: ConsoleDisplaySource;
    phase: "reconnecting" | "disconnected";
    reason: string;
  };

const MAX_CONSOLE_REASON_LENGTH = 240;

export function boundedConsoleReason(
  reason: string | null | undefined,
  fallback: string,
): string {
  const normalized = reason?.replace(/\s+/g, " ").trim() || fallback;
  return normalized.slice(0, MAX_CONSOLE_REASON_LENGTH);
}

export function displayedConsoleSource(
  state: ConsoleDisplayState,
): ConsoleDisplaySource | null {
  switch (state.kind) {
    case "ready":
    case "stale":
      return state.source;
    case "loading":
    case "unavailable":
    case "failed":
      return null;
  }
}
