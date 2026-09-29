import type {
  SessionConversationTurn,
  SessionHistoryPage,
  SessionSnapshotEntry,
} from "$lib/generated/protocol";
import {
  applyConsoleHistoryPage,
  beginConsoleHistoryRequest,
  type ConsoleHistoryState,
  conversationTurnPreviews,
  emptyConsoleHistoryState,
  historyEntries,
  setConsoleHistoryTopEdge,
  shouldLoadHistoryAtTop,
} from "./history";

declare const Deno: { test(name: string, fn: () => void): void };

function assert(condition: boolean, message: string): void {
  if (!condition) throw new Error(message);
}

function message(
  id: string,
  role: "user" | "assistant",
  text: string,
): SessionSnapshotEntry {
  return {
    entry_id: id,
    timestamp: 1,
    provenance: role === "user" ? "human_input" : "model_output",
    kind: "message",
    role,
    content: [{ kind: "text", text }],
  };
}

function turn(index: number): SessionConversationTurn {
  return {
    turn_id: `u-${index}`,
    entries: [
      message(`u-${index}`, "user", `question ${index}\nignored`),
      message(`a-progress-${index}`, "assistant", `working ${index}`),
      message(
        `a-final-${index}`,
        "assistant",
        `done ${index}\ndetail a\ndetail b\ndetail c`,
      ),
    ],
  };
}

function page(
  turns: SessionConversationTurn[],
  cursor: string | null,
): SessionHistoryPage {
  return {
    session_id: "session-a",
    lineage_id: "lineage-a",
    turns,
    next_cursor: cursor,
    has_more: cursor !== null,
  };
}

Deno.test("history prepends 12 turns as 5 then 5 then 2 without duplicates", () => {
  let state = emptyConsoleHistoryState();
  state = beginConsoleHistoryRequest(state, null)!;
  state = applyConsoleHistoryPage(
    state,
    page([8, 9, 10, 11, 12].map(turn), "cursor-8"),
    null,
  );
  state = beginConsoleHistoryRequest(state, "cursor-8")!;
  state = applyConsoleHistoryPage(
    state,
    page([3, 4, 5, 6, 7].map(turn), "cursor-3"),
    "cursor-8",
  );
  state = beginConsoleHistoryRequest(state, "cursor-3")!;
  state = applyConsoleHistoryPage(
    state,
    page([1, 2].map(turn), null),
    "cursor-3",
  );
  assert(state.turns.length === 12, "all real turns must be retained");
  assert(state.turns[0].turn_id === "u-1", "pages must remain chronological");
  assert(
    !state.hasMore && state.cursor === null,
    "final short page must end history",
  );

  state = { ...state, requestedCursor: "__initial__", status: "loading" };
  state = applyConsoleHistoryPage(
    state,
    page([8, 9, 10, 11, 12].map(turn), "cursor-8"),
    null,
  );
  assert(
    state.turns.length === 12,
    "refresh must dedupe stable turn identities",
  );
  assert(
    historyEntries(state).length === 36,
    "entries must dedupe by stable identity",
  );
});

Deno.test("refresh replaces overlapping latest turns while retaining loaded earlier turns", () => {
  let state = emptyConsoleHistoryState();
  state = beginConsoleHistoryRequest(state, null)!;
  state = applyConsoleHistoryPage(
    state,
    page([8, 9, 10, 11, 12].map(turn), "cursor-8"),
    null,
  );
  state = beginConsoleHistoryRequest(state, "cursor-8")!;
  state = applyConsoleHistoryPage(
    state,
    page([3, 4, 5, 6, 7].map(turn), "cursor-3"),
    "cursor-8",
  );

  const updated = turn(12);
  updated.entries.push(message("a-later-12", "assistant", "actually finished"));
  state = beginConsoleHistoryRequest(state, null)!;
  state = applyConsoleHistoryPage(
    state,
    page([8, 9, 10, 11].map(turn).concat(updated), "stale-cursor-is-ignored"),
    null,
  );

  assert(
    state.turns.length === 10,
    "refresh must not discard loaded earlier turns",
  );
  assert(
    state.turns.at(-1)?.entries.at(-1)?.entry_id === "a-later-12",
    "refresh must replace an overlapping turn with its latest committed entries",
  );
  assert(
    state.cursor === "cursor-3",
    "refresh must retain the oldest paging boundary",
  );
});

Deno.test("turn preview uses one user line and final non-empty assistant up to three lines", () => {
  const preview = conversationTurnPreviews([turn(4)])[0];
  assert(preview.user === "question 4", "user preview must use its first line");
  assert(
    preview.assistant.join("|") === "done 4|detail a|detail b",
    "preview must choose the final assistant and cap it at three lines",
  );
});

Deno.test("top sentinel requests only once until it leaves the edge", () => {
  let state: ConsoleHistoryState = {
    ...emptyConsoleHistoryState(),
    status: "ready",
    cursor: "older",
    hasMore: true,
  };
  assert(
    shouldLoadHistoryAtTop(state, true),
    "armed top edge should request a page",
  );
  state = beginConsoleHistoryRequest(state, "older")!;
  assert(
    !shouldLoadHistoryAtTop(state, true),
    "in-flight page must suppress duplicates",
  );
  state = {
    ...state,
    status: "ready",
    requestedCursor: null,
  };
  assert(
    !shouldLoadHistoryAtTop(state, true),
    "stationary sentinel must not drain history",
  );
  state = setConsoleHistoryTopEdge(state, false);
  assert(
    shouldLoadHistoryAtTop(state, true),
    "leaving the top must rearm one request",
  );
});
