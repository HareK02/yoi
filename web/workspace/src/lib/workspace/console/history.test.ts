import type {
  SessionConversationTurn,
  SessionHistoryPage,
  SessionSnapshotEntry,
} from "#lib/generated/protocol.ts";
import {
  applyConsoleHistoryPage,
  beginConsoleHistoryRequest,
  type ConsoleHistoryState,
  conversationTurnPreviews,
  conversationTurnPreviewsFromLines,
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
  lineageId = "lineage-a",
  compactAncestorLineageIds: string[] = [],
  parentLineage?: {
    lineage_id: string;
    adopted_through_turn?: SessionConversationTurn | null;
  },
): SessionHistoryPage {
  return {
    session_id: "session-a",
    lineage_id: lineageId,
    compact_ancestor_lineage_ids: compactAncestorLineageIds,
    parent_lineage: parentLineage,
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

Deno.test("same-lineage refresh without overlap replaces the paging boundary", () => {
  let state: ConsoleHistoryState = {
    ...emptyConsoleHistoryState(),
    sessionId: "session-a",
    lineageId: "lineage-a",
    turns: [1, 2, 3, 4].map(turn),
    cursor: null,
    hasMore: false,
    status: "loading",
    requestedCursor: "__initial__",
  };
  state = applyConsoleHistoryPage(
    state,
    page([6, 7, 8, 9, 10].map(turn), "cursor-6", "lineage-a"),
    null,
  );

  assert(
    state.turns.map((value) => value.turn_id).join(",") ===
      "u-1,u-2,u-3,u-4,u-6,u-7,u-8,u-9,u-10",
    "a reconnect may preserve loaded rows while still exposing the missing gap",
  );
  assert(
    state.cursor === "cursor-6" && state.hasMore,
    "a non-overlapping same-lineage page must replace an exhausted old boundary",
  );
});

Deno.test("changed lineage keeps the adopted prefix and discards the replaced branch", () => {
  let state = emptyConsoleHistoryState();
  state = beginConsoleHistoryRequest(state, null)!;
  state = applyConsoleHistoryPage(
    state,
    page([6, 7, 8, 9, 10].map(turn), "cursor-6"),
    null,
  );
  state = beginConsoleHistoryRequest(state, "cursor-6")!;
  state = applyConsoleHistoryPage(
    state,
    page([1, 2, 3, 4, 5].map(turn), null),
    "cursor-6",
  );

  state = beginConsoleHistoryRequest(state, null)!;
  state = applyConsoleHistoryPage(
    state,
    page([3, 4, 5, 11, 12].map(turn), "cursor-3-new", "lineage-b"),
    null,
  );

  assert(
    state.turns.map((value) => value.turn_id).join(",") ===
      "u-1,u-2,u-3,u-4,u-5,u-11,u-12",
    "rewind must preserve only the prefix adopted by the new lineage",
  );
  assert(
    state.cursor === "cursor-3-new" && state.hasMore,
    "a changed lineage must replace the stale paging cursor",
  );
});

Deno.test("Compact lineage refresh preserves already loaded adopted history", () => {
  let state: ConsoleHistoryState = {
    ...emptyConsoleHistoryState(),
    sessionId: "session-a",
    lineageId: "lineage-a",
    turns: Array.from({ length: 10 }, (_, index) => turn(index + 1)),
    status: "loading",
    requestedCursor: "__initial__",
  };
  state = applyConsoleHistoryPage(
    state,
    page(
      [11, 12, 13, 14, 15].map(turn),
      "cursor-11-new",
      "lineage-compact",
      ["lineage-a"],
    ),
    null,
  );

  assert(
    state.turns.map((value) => value.turn_id).join(",") ===
      Array.from({ length: 15 }, (_, index) => `u-${index + 1}`).join(","),
    "Compact must keep the adopted prefix without requiring newest-page overlap",
  );
  assert(
    state.cursor === "cursor-11-new" && state.hasMore,
    "Compact must replace the old-lineage cursor even when adopted turns are preserved",
  );
});

Deno.test("fork lineage boundary preserves an adopted prefix without page overlap", () => {
  let state: ConsoleHistoryState = {
    ...emptyConsoleHistoryState(),
    sessionId: "session-a",
    lineageId: "lineage-a",
    turns: Array.from({ length: 50 }, (_, index) => turn(index + 1)),
    cursor: null,
    hasMore: false,
    status: "loading",
    requestedCursor: "__initial__",
  };
  state.turns[24]!.entries.push(
    message("a-unadopted-25", "assistant", "stale sibling response"),
  );
  const adoptedBoundary = turn(25);
  state = applyConsoleHistoryPage(
    state,
    page(
      [101, 102, 103, 104, 105].map(turn),
      "cursor-101",
      "lineage-fork",
      [],
      {
        lineage_id: "lineage-a",
        adopted_through_turn: adoptedBoundary,
      },
    ),
    null,
  );

  assert(
    state.turns.map((value) => value.turn_id).join(",") ===
      [
        ...Array.from({ length: 25 }, (_, index) => `u-${index + 1}`),
        ...Array.from({ length: 5 }, (_, index) => `u-${index + 101}`),
      ].join(","),
    "fork refresh must retain only the authoritative adopted prefix",
  );
  assert(
    historyEntries(state).every((entry) => entry.entry_id !== "a-unadopted-25"),
    "the canonical provider boundary must trim a sibling entry in the same visible turn",
  );
  assert(
    state.cursor === "cursor-101" && state.hasMore,
    "fork refresh must replace the old paging boundary",
  );
});

Deno.test("changed lineage without a stable overlap replaces retained turns", () => {
  let state: ConsoleHistoryState = {
    ...emptyConsoleHistoryState(),
    sessionId: "session-a",
    lineageId: "lineage-a",
    turns: [1, 2, 3].map(turn),
    cursor: "old-cursor",
    hasMore: true,
    status: "loading",
    requestedCursor: "__initial__",
  };
  state = applyConsoleHistoryPage(
    state,
    page([11, 12].map(turn), null, "lineage-b"),
    null,
  );
  assert(
    state.turns.map((value) => value.turn_id).join(",") === "u-11,u-12",
    "unrelated lineage content must not survive refresh",
  );
  assert(
    state.cursor === null && !state.hasMore,
    "new lineage boundary must be authoritative",
  );
});

Deno.test("line previews include a current turn outside the retained five-turn page", () => {
  const lines = [1, 2, 3, 4, 5, 6].flatMap((index) => [
    {
      id: `user-${index}`,
      entryId: `u-${index}`,
      kind: "user" as const,
      title: "User",
      body: `question ${index}`,
      source: "event" as const,
    },
    {
      id: `assistant-${index}`,
      entryId: `a-${index}`,
      kind: "assistant" as const,
      title: "Assistant",
      body: `done ${index}`,
      source: "event" as const,
    },
  ]);
  assert(
    conversationTurnPreviewsFromLines(lines).map((preview) => preview.turnId)
      .join(",") ===
      "u-1,u-2,u-3,u-4,u-5,u-6",
    "turn navigation must include unmatched current snapshot turns in transcript order",
  );
});

Deno.test("line previews track a snapshot-restored assistant as its body streams", () => {
  const lines = [{
    id: "user",
    entryId: "user-entry",
    kind: "user" as const,
    title: "User",
    body: "question",
    source: "event" as const,
  }, {
    id: "restored-assistant",
    kind: "assistant" as const,
    title: "assistant streaming",
    body: "**hel",
    source: "event" as const,
    streaming: true,
  }];

  assert(
    conversationTurnPreviewsFromLines(lines)[0].assistant.join("|") ===
      "**hel",
    "restored assistant prefix must use the ordinary assistant preview path",
  );
  const continued = [lines[0], { ...lines[1], body: "**hello**" }];
  assert(
    conversationTurnPreviewsFromLines(continued)[0].assistant.join("|") ===
      "**hello**",
    "live suffixes must update the same turn preview",
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
