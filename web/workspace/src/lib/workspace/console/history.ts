import type {
  SessionConversationTurn,
  SessionHistoryPage,
  SessionSnapshotEntry,
} from "$lib/generated/protocol";

export const CONSOLE_HISTORY_PAGE_TURNS = 5;

export type ConsoleHistoryStatus = "idle" | "loading" | "ready" | "error";

export type ConsoleHistoryState = {
  sessionId: string | null;
  lineageId: string | null;
  turns: SessionConversationTurn[];
  cursor: string | null;
  hasMore: boolean;
  status: ConsoleHistoryStatus;
  error: string | null;
  /** Cursor currently in flight; prevents duplicate sentinel requests. */
  requestedCursor: string | null;
  /** Rearmed only after leaving the top edge or an explicit user action. */
  topEdgeArmed: boolean;
};

export type ConsoleTurnPreview = {
  turnId: string;
  lineId: string;
  user: string;
  assistant: string[];
};

export function emptyConsoleHistoryState(): ConsoleHistoryState {
  return {
    sessionId: null,
    lineageId: null,
    turns: [],
    cursor: null,
    hasMore: false,
    status: "idle",
    error: null,
    requestedCursor: null,
    topEdgeArmed: true,
  };
}

export function beginConsoleHistoryRequest(
  state: ConsoleHistoryState,
  cursor: string | null,
): ConsoleHistoryState | null {
  if (state.status === "loading" || state.requestedCursor !== null) return null;
  if (cursor !== null && (!state.hasMore || cursor !== state.cursor)) {
    return null;
  }
  return {
    ...state,
    status: "loading",
    error: null,
    requestedCursor: cursor ?? "__initial__",
    topEdgeArmed: cursor === null ? state.topEdgeArmed : false,
  };
}

export function applyConsoleHistoryPage(
  state: ConsoleHistoryState,
  page: SessionHistoryPage,
  requestedCursor: string | null,
): ConsoleHistoryState {
  const requestKey = requestedCursor ?? "__initial__";
  if (state.requestedCursor !== requestKey) return state;

  const initial = requestedCursor === null;
  if (
    !initial &&
    (state.sessionId !== page.session_id || state.lineageId !== page.lineage_id)
  ) {
    return failConsoleHistoryRequest(
      state,
      "The conversation changed while earlier history was loading. Retry from the current view.",
    );
  }

  const incoming = page.turns.filter((turn) => turn.entries.length > 0);
  const refreshingSameSession = initial &&
    state.sessionId === page.session_id &&
    state.turns.length > 0;
  const turns = initial
    ? refreshingSameSession
      ? mergeRefreshedTurns(state.turns, incoming)
      : dedupeTurns(incoming)
    : dedupeTurns([...incoming, ...state.turns]);
  return {
    ...state,
    sessionId: page.session_id,
    lineageId: page.lineage_id,
    turns,
    cursor: refreshingSameSession ? state.cursor : (page.next_cursor ?? null),
    hasMore: refreshingSameSession ? state.hasMore : page.has_more,
    status: "ready",
    error: null,
    requestedCursor: null,
  };
}

export function failConsoleHistoryRequest(
  state: ConsoleHistoryState,
  message: string,
): ConsoleHistoryState {
  return {
    ...state,
    status: "error",
    error: message.slice(0, 512),
    requestedCursor: null,
  };
}

export function setConsoleHistoryTopEdge(
  state: ConsoleHistoryState,
  atTop: boolean,
): ConsoleHistoryState {
  if (!atTop && !state.topEdgeArmed) return { ...state, topEdgeArmed: true };
  return state;
}

export function shouldLoadHistoryAtTop(
  state: ConsoleHistoryState,
  atTop: boolean,
): boolean {
  return atTop && state.topEdgeArmed && state.hasMore &&
    state.status !== "loading" && state.requestedCursor === null;
}

export function historyEntries(
  state: ConsoleHistoryState,
): SessionSnapshotEntry[] {
  return state.turns.flatMap((turn) => turn.entries);
}

export function conversationTurnPreviews(
  turns: readonly SessionConversationTurn[],
): ConsoleTurnPreview[] {
  return turns.map((turn) => {
    const userEntry = turn.entries.find(isUserEntry);
    const assistants = turn.entries
      .filter(isAssistantEntry)
      .map(entryText)
      .filter((text) => text.trim().length > 0);
    const assistant = (assistants.at(-1) ?? "")
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(Boolean)
      .slice(0, 3);
    return {
      turnId: turn.turn_id,
      lineId: userEntry?.entry_id ?? turn.turn_id,
      user: firstNonEmptyLine(userEntry ? entryText(userEntry) : ""),
      assistant,
    };
  });
}

function mergeRefreshedTurns(
  retained: readonly SessionConversationTurn[],
  refreshed: readonly SessionConversationTurn[],
): SessionConversationTurn[] {
  const refreshedIds = new Set(refreshed.map((turn) => turn.turn_id));
  return dedupeTurns([
    ...retained.filter((turn) => !refreshedIds.has(turn.turn_id)),
    ...refreshed,
  ]);
}

function dedupeTurns(
  turns: readonly SessionConversationTurn[],
): SessionConversationTurn[] {
  const seenTurns = new Set<string>();
  const seenEntries = new Set<string>();
  const result: SessionConversationTurn[] = [];
  for (const turn of turns) {
    if (seenTurns.has(turn.turn_id)) continue;
    const entries = turn.entries.filter((entry) => {
      if (seenEntries.has(entry.entry_id)) return false;
      seenEntries.add(entry.entry_id);
      return true;
    });
    if (entries.length === 0) continue;
    seenTurns.add(turn.turn_id);
    result.push({ ...turn, entries });
  }
  return result;
}

function isUserEntry(entry: SessionSnapshotEntry): boolean {
  return entry.kind === "user_input" ||
    (entry.kind === "message" && entry.role === "user");
}

function isAssistantEntry(entry: SessionSnapshotEntry): boolean {
  return entry.kind === "message" && entry.role === "assistant";
}

function entryText(entry: SessionSnapshotEntry): string {
  switch (entry.kind) {
    case "user_input":
      return entry.segments.map((segment) => {
        if (segment.kind === "text" || segment.kind === "paste") {
          return segment.content;
        }
        if (segment.kind === "file_ref") return `@file ${segment.path}`;
        if (segment.kind === "uploaded_file") {
          return `[Attachment: ${segment.file.file_name}]`;
        }
        if (segment.kind === "paste_artifact") {
          return `[Large paste ${segment.artifact.artifact_id}]`;
        }
        return "";
      }).filter(Boolean).join("\n");
    case "message":
      return entry.content.map((part) =>
        part.kind === "text" ? part.text : part.refusal
      ).join("\n");
    default:
      return "";
  }
}

function firstNonEmptyLine(value: string): string {
  return value.split(/\r?\n/).map((line) => line.trim()).find(Boolean) ?? "—";
}
