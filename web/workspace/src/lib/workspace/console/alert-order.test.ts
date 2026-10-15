import type {
  Alert,
  AlertPosition,
  Event,
  InFlightSnapshot,
  SessionSnapshotEntry,
} from "#lib/generated/protocol.ts";
import {
  applyProtocolEvent,
  type ConsoleLine,
  type ConsoleProjection,
  emptyConsoleProjection,
  mergeCommittedHistoryLines,
  projectConsoleLines,
  projectSessionHistoryEntries,
  projectVisibleConsole,
} from "./model.ts";

declare const Deno: { test(name: string, fn: () => void): void };

function assertEquals(actual: unknown, expected: unknown): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `Expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

let sequence = 0;
function apply(
  projection: ConsoleProjection,
  event: Event,
  observedAtMs = 10000,
): ConsoleProjection {
  return applyProtocolEvent(projection, {
    eventId: `event-${sequence++}`,
    event,
    observedAtMs,
  });
}

function message(
  id: string,
  timestamp: number,
  body = id,
): SessionSnapshotEntry {
  return {
    kind: "message",
    entry_id: id,
    timestamp,
    provenance: "model_output",
    derived_from: [],
    role: "assistant",
    content: [{ kind: "text", text: body }],
  };
}

function readCall(id: string, timestamp: number): SessionSnapshotEntry {
  return {
    kind: "tool_call",
    entry_id: `entry-${id}`,
    timestamp,
    provenance: "model_output",
    derived_from: [],
    call_id: id,
    name: "Read",
    arguments: JSON.stringify({ file_path: `${id}.ts` }),
  };
}

function snapshot(
  entries: SessionSnapshotEntry[],
  inFlight?: InFlightSnapshot,
): Event {
  return {
    event: "snapshot",
    data: {
      greeting: {
        worker_name: "Worker",
        cwd: "/workspace",
        provider: "provider",
        model: "model",
        scope_summary: "bounded",
        tools: [],
        context_window: 100,
        context_tokens: 20,
      },
      state: { last_command_id: 0, state: { kind: "idle" } },
      session: {
        entries,
        pending_submissions: {
          notification_count: 0,
          head_id: null,
          submissions: [],
        },
      },
      in_flight: inFlight,
      internal_workers: [],
    },
  };
}

function alert(
  id: string | undefined,
  position?: AlertPosition,
  timestamp = 150,
): Alert {
  return {
    id,
    position,
    level: "warn",
    source: "worker",
    message: "same warning",
    timestamp_ms: timestamp,
  };
}

function emit(projection: ConsoleProjection, data: Alert): ConsoleProjection {
  return apply(projection, { event: "alert", data });
}

function commit(
  projection: ConsoleProjection,
  entry: SessionSnapshotEntry,
): ConsoleProjection {
  return apply(projection, {
    event: "session_entry_committed",
    data: { entry },
  });
}

function labels(lines: ConsoleLine[]): string[] {
  return lines.map((line) =>
    line.alert
      ? `alert:${line.alert.id ?? "legacy"}`
      : line.toolCall?.id ?? line.body
  );
}

function visible(projection: ConsoleProjection): string[] {
  return labels(projectVisibleConsole(projection).lines);
}

Deno.test("live Alert chronology equals fresh snapshot plus replay without timestamp sorting", () => {
  const entries = [
    message("first", 300),
    message("second", 100),
    message("third", 200),
  ];
  const warnings = [
    alert("start", { kind: "start" }),
    alert("a", { kind: "after_entry", entry_id: "first" }),
    alert("b", { kind: "after_entry", entry_id: "first" }),
    alert("c", { kind: "after_entry", entry_id: "second" }),
  ];
  let live = emit(emptyConsoleProjection(), warnings[0]);
  live = commit(live, entries[0]);
  live = emit(emit(live, warnings[1]), warnings[2]);
  live = emit(commit(live, entries[1]), warnings[3]);
  live = commit(live, entries[2]);
  let replay = apply(emptyConsoleProjection(), snapshot(entries));
  for (const warning of warnings) replay = emit(replay, warning);
  assertEquals(visible(live), [
    "alert:start",
    "first",
    "alert:a",
    "alert:b",
    "second",
    "alert:c",
    "third",
  ]);
  assertEquals(visible(replay), visible(live));
});

Deno.test("same-id replay updates metadata; distinct same-ms same-body IDs remain distinct", () => {
  const entries = [message("one", 100), message("two", 200)];
  let projection = apply(emptyConsoleProjection(), snapshot(entries));
  projection = emit(
    projection,
    alert("a", { kind: "after_entry", entry_id: "one" }),
  );
  projection = emit(
    projection,
    alert("b", { kind: "after_entry", entry_id: "one" }),
  );
  const originalId =
    projection.lines.find((line) => line.alert?.id === "a")!.id;
  projection = apply(projection, snapshot(entries));
  assertEquals(visible(projection), ["one", "alert:a", "alert:b", "two"]);
  projection = emit(
    projection,
    alert("a", { kind: "after_entry", entry_id: "two" }),
  );
  projection = emit(
    projection,
    alert("a", { kind: "after_entry", entry_id: "two" }),
  );
  projection = emit(
    projection,
    alert("b", { kind: "after_entry", entry_id: "one" }),
  );
  assertEquals(visible(projection), ["one", "alert:b", "two", "alert:a"]);
  assertEquals(
    projection.lines.find((line) => line.alert?.id === "a")!.id,
    originalId,
  );
  assertEquals(projection.lines.filter((line) => line.alert).length, 2);
});

Deno.test("live pending Alert anchor follows the actual assistant through commit and reconnect", () => {
  let live = commit(emptyConsoleProjection(), message("old", 10));
  live = apply(live, { event: "text_delta", data: { text: "pending" } }, 100);
  live = emit(
    live,
    alert("pending-alert", { kind: "after_in_flight", block_id: 42 }),
  );
  assertEquals(visible(live), ["old", "pending", "alert:pending-alert"]);
  assertEquals(
    live.lines.find((line) => line.body === "pending")?.inFlightBlockId,
    42,
  );
  live = apply(
    live,
    { event: "text_done", data: { text: "pending answer" } },
    180,
  );
  live = commit(live, message("answer", 190, "pending answer"));
  live = commit(live, message("later", 200));
  assertEquals(visible(live), [
    "old",
    "pending answer",
    "alert:pending-alert",
    "later",
  ]);
  assertEquals(live.lines.find((line) => line.alert)?.alert?.position, {
    kind: "after_entry",
    entry_id: "answer",
  });
  assertEquals(
    live.lines.find((line) => line.entryId === "answer")?.inFlightBlockId,
    42,
  );
  // Even a repeated pending-anchor payload cannot bind to a later assistant.
  live = emit(
    live,
    alert("pending-alert", { kind: "after_in_flight", block_id: 42 }),
  );
  assertEquals(visible(live), [
    "old",
    "pending answer",
    "alert:pending-alert",
    "later",
  ]);
  const entries = [
    message("old", 10),
    message("answer", 190, "pending answer"),
    message("later", 200),
  ];
  live = apply(live, snapshot(entries));
  assertEquals(visible(live), [
    "old",
    "pending answer",
    "alert:pending-alert",
    "later",
  ]);
  const resolved = alert("pending-alert", {
    kind: "after_entry",
    entry_id: "answer",
  });
  live = emit(live, resolved);
  const fresh = emit(
    apply(emptyConsoleProjection(), snapshot(entries)),
    resolved,
  );
  assertEquals(visible(live), visible(fresh));
});

Deno.test("snapshot block IDs restore pending anchors and survive text continuation", () => {
  const entries = [message("old", 10)];
  const inFlight: InFlightSnapshot = {
    block_ids: [7, 8, 9],
    blocks: [
      { kind: "text", text: "first pending", finished: true },
      {
        kind: "tool_call",
        id: "read",
        name: "Read",
        args: "{}",
        state: "done",
      },
      { kind: "text", text: "second", finished: false },
    ],
  };
  let projection = apply(emptyConsoleProjection(), snapshot(entries, inFlight));
  projection = emit(
    projection,
    alert("a", { kind: "after_in_flight", block_id: 7 }),
  );
  projection = emit(
    projection,
    alert("b", { kind: "after_in_flight", block_id: 9 }),
  );
  const before = visible(projection);
  projection = apply(projection, snapshot(entries, inFlight));
  assertEquals(visible(projection), before);
  assertEquals(before, [
    "old",
    "first pending",
    "alert:a",
    "read",
    "second",
    "alert:b",
  ]);
  projection = commit(projection, message("first", 100, "first pending"));
  projection = apply(projection, {
    event: "text_delta",
    data: { text: " pending" },
  });
  projection = apply(projection, {
    event: "text_done",
    data: { text: "second pending" },
  });
  projection = commit(projection, message("second", 200, "second pending"));
  assertEquals(visible(projection), [
    "old",
    "first pending",
    "alert:a",
    "read",
    "second pending",
    "alert:b",
  ]);
  assertEquals(
    projection.lines.filter((line) => line.alert).map((line) =>
      line.alert!.position
    ),
    [
      { kind: "after_entry", entry_id: "first" },
      { kind: "after_entry", entry_id: "second" },
    ],
  );
});

Deno.test("snapshot with committed pending text retains Alerts until replay resolves metadata", () => {
  let projection = apply(emptyConsoleProjection(), {
    event: "text_delta",
    data: { text: "answer" },
  });
  projection = emit(
    projection,
    alert("a", { kind: "after_in_flight", block_id: 15 }),
  );
  projection = apply(
    projection,
    snapshot([message("answer", 100), message("later", 200)]),
  );
  assertEquals(projection.lines.filter((line) => line.alert).length, 1);
  projection = emit(
    projection,
    alert("a", { kind: "after_entry", entry_id: "answer" }),
  );
  assertEquals(visible(projection), ["answer", "alert:a", "later"]);
});

Deno.test("Alert anchors split Read and overview aggregation before and after history merge", () => {
  const entries = [
    readCall("r1", 100),
    readCall("r2", 200),
    readCall("r3", 300),
  ];
  let projection = apply(emptyConsoleProjection(), snapshot(entries));
  projection = emit(
    projection,
    alert("boundary", { kind: "after_tool_call", call_id: "r1" }),
  );
  const lines = projectVisibleConsole(projection).lines;
  assertEquals(
    lines.map((line) => line.alert ? "alert" : line.toolCallCount ?? 1),
    [1, "alert", 2],
  );
  assertEquals(
    projectConsoleLines(lines, "overview").map((line) => line.kind),
    ["activity", "status", "activity"],
  );
  const history = projectSessionHistoryEntries(entries, null);
  const merged = mergeCommittedHistoryLines(history, lines);
  assertEquals(
    merged.map((line) => line.alert ? "alert" : line.toolCallCount ?? 1),
    [1, "alert", 2],
  );
  assertEquals(
    projectConsoleLines(merged, "overview").map((line) => line.kind),
    ["activity", "status", "activity"],
  );
});

Deno.test("missing older anchors stay before the window and resolve when history arrives", () => {
  let projection = apply(
    emptyConsoleProjection(),
    snapshot([message("current", 300)]),
  );
  projection = emit(
    projection,
    alert("older", { kind: "after_entry", entry_id: "older" }),
  );
  projection = emit(
    projection,
    alert("same-anchor", { kind: "after_entry", entry_id: "older" }),
  );
  projection = emit(projection, alert("start", { kind: "start" }));
  const current = projectVisibleConsole(projection).lines;
  assertEquals(labels(current), [
    "alert:older",
    "alert:same-anchor",
    "alert:start",
    "current",
  ]);
  const history = projectSessionHistoryEntries([
    message("first", 50),
    message("older", 100),
    message("middle", 200),
    message("current", 300),
  ], null);
  const merged = mergeCommittedHistoryLines(history, current);
  assertEquals(labels(merged), [
    "alert:start",
    "first",
    "older",
    "alert:older",
    "alert:same-anchor",
    "middle",
    "current",
  ]);
  assertEquals(
    labels(mergeCommittedHistoryLines(history, merged)),
    labels(merged),
  );
});

Deno.test("legacy Alerts use original emission timestamps for live and replay chronology", () => {
  let live = apply(emptyConsoleProjection(), {
    event: "text_done",
    data: { text: "first" },
  }, 100);
  live = emit(live, alert(undefined, undefined, 150));
  live = apply(live, { event: "text_done", data: { text: "second" } }, 200);
  assertEquals(visible(live), ["first", "alert:legacy", "second"]);
  let replay = apply(
    emptyConsoleProjection(),
    snapshot([message("first", 100), message("second", 200)]),
  );
  replay = emit(replay, alert(undefined, undefined, 150));
  replay = emit(replay, alert(undefined, undefined, 150));
  assertEquals(visible(replay), visible(live));
  assertEquals(replay.lines.map((line) => line.timestampMs), [100, 200, 150]);
  const merged = mergeCommittedHistoryLines(
    projectSessionHistoryEntries([message("earliest", 50)], null),
    projectVisibleConsole(replay).lines,
  );
  assertEquals(labels(merged), ["earliest", "first", "alert:legacy", "second"]);
});

Deno.test("appended entry metadata does not rescan prior event IDs or mutate retained rows", () => {
  const projection = apply(
    emptyConsoleProjection(),
    snapshot(
      Array.from(
        { length: 128 },
        (_, index) => message(`entry-${index}`, index),
      ),
    ),
  );
  let priorEventIdReads = 0;
  projection.lines = projection.lines.map((line) =>
    Object.freeze({
      ...line,
      get eventId() {
        priorEventIdReads += 1;
        return line.eventId;
      },
    })
  );
  const next = commit(projection, message("appended", 200));
  assertEquals(priorEventIdReads, 0);
  assertEquals(next.lines.length, projection.lines.length + 1);
  assertEquals(
    next.lines.slice(0, -1).every((line, index) =>
      line === projection.lines[index]
    ),
    true,
  );
  assertEquals(next.lines.at(-1)?.entryId, "appended");
  assertEquals(next.lines.at(-1)?.sourceEntryIds, ["appended"]);
  assertEquals(next.lines.at(-1)?.timestampMs, 200);
});

Deno.test("snapshot retention indexes previous entry metadata in one pass", () => {
  const entries = Array.from(
    { length: 128 },
    (_, index) => message(`entry-${index}`, index),
  );
  const projection = apply(emptyConsoleProjection(), snapshot(entries));
  let priorEntryIdReads = 0;
  projection.lines = projection.lines.map((line, index) =>
    Object.freeze({
      ...line,
      inFlightBlockId: index,
      get entryId() {
        priorEntryIdReads += 1;
        return line.entryId;
      },
    })
  );
  const next = apply(projection, snapshot(entries));
  assertEquals(priorEntryIdReads, entries.length);
  assertEquals(
    next.lines.map((line) => line.inFlightBlockId),
    entries.map((_, index) => index),
  );
  assertEquals(
    next.lines.map((line) => line.timestampMs),
    entries.map((_, index) => index),
  );
});

Deno.test("tool-result reconciliation retains call-entry anchors and creation timestamp", () => {
  let projection = commit(emptyConsoleProjection(), readCall("r1", 100));
  projection = emit(
    projection,
    alert("a", { kind: "after_entry", entry_id: "entry-r1" }),
  );
  projection = commit(projection, readCall("r2", 200));
  const before = projection;
  const originalCall = before.lines[0];
  Object.freeze(originalCall.sourceEntryIds);
  Object.freeze(originalCall);
  projection = commit(projection, {
    kind: "tool_result",
    entry_id: "result-r1",
    timestamp: 250,
    provenance: "tool_output",
    derived_from: [],
    call_id: "r1",
    summary: "read",
    content: "text",
    is_error: false,
  });
  assertEquals(visible(projection), ["r1", "alert:a", "r2"]);
  assertEquals(originalCall.sourceEntryIds, ["entry-r1"]);
  assertEquals(originalCall.entryId, "entry-r1");
  assertEquals(originalCall.toolCall?.state, "running");
  assertEquals(projection.lines[0] === originalCall, false);
  assertEquals(projection.lines[2] === before.lines[2], true);
  assertEquals(projection.lines[0].sourceEntryIds, ["entry-r1", "result-r1"]);
  assertEquals(
    projection.lines.find((line) => line.toolCall?.id === "r1")?.timestampMs,
    100,
  );
  projection = emit(
    projection,
    alert("a", { kind: "after_entry", entry_id: "result-r1" }),
  );
  assertEquals(visible(projection), ["r1", "alert:a", "r2"]);
});

Deno.test("history enriches a live tool row with durable anchors before placing Alerts", () => {
  let projection = apply(emptyConsoleProjection(), {
    event: "tool_call_start",
    data: { id: "r1", name: "Read" },
  }, 110);
  projection = emit(
    projection,
    alert("a", { kind: "after_entry", entry_id: "entry-r1" }),
  );
  projection = apply(projection, {
    event: "tool_call_start",
    data: { id: "r2", name: "Read" },
  }, 210);
  const merged = mergeCommittedHistoryLines(
    projectSessionHistoryEntries(
      [readCall("r1", 100), readCall("r2", 200)],
      null,
    ),
    projectVisibleConsole(projection).lines,
  );
  assertEquals(labels(merged), ["r1", "alert:a", "r2"]);
  assertEquals(merged[0].toolCallLines?.[0].timestampMs, 100);
  assertEquals(merged[0].toolCallLines?.[0].toolCall?.state, "pending");
});

Deno.test("live pending metadata survives reconnect before commit without changing first observation", () => {
  let projection = apply(emptyConsoleProjection(), {
    event: "text_delta",
    data: { text: "pending" },
  }, 100);
  const warning = alert("a", { kind: "after_in_flight", block_id: 27 });
  projection = emit(projection, warning);
  projection = apply(
    projection,
    snapshot([], {
      block_ids: [27],
      blocks: [{ kind: "text", text: "pending", finished: false }],
    }),
    5000,
  );
  assertEquals(visible(projection), ["pending", "alert:a"]);
  assertEquals(projection.lines[0].timestampMs, 100);
  projection = emit(projection, warning);
  assertEquals(visible(projection), ["pending", "alert:a"]);
  projection = apply(projection, {
    event: "text_done",
    data: { text: "pending" },
  });
  projection = commit(projection, message("answer", 200, "pending"));
  assertEquals(projection.lines.find((line) => line.alert)?.alert?.position, {
    kind: "after_entry",
    entry_id: "answer",
  });
});

Deno.test("parent snapshot retains child Alerts in their own console only", () => {
  const worker = {
    kind: "sub_worker",
    name: "child",
    session_id: "child-session",
  } as const;
  const childEvent = (event: Event): Event => ({
    event: "internal_worker",
    data: { worker, event },
  });
  let projection = apply(
    emptyConsoleProjection(),
    childEvent(snapshot([message("child-answer", 100)])),
  );
  projection = apply(
    projection,
    childEvent({
      event: "alert",
      data: alert("a", { kind: "after_entry", entry_id: "child-answer" }),
    }),
  );
  const parentSnapshot = snapshot([message("parent", 300)]);
  if (parentSnapshot.event !== "snapshot") throw new Error("snapshot expected");
  const childSnapshot = snapshot([message("child-answer", 100)]);
  if (childSnapshot.event !== "snapshot") throw new Error("snapshot expected");
  parentSnapshot.data.internal_workers = [{
    worker,
    status: "idle",
    error: null,
    session: childSnapshot.data.session,
  }];
  projection = apply(projection, parentSnapshot);
  assertEquals(visible(projection), ["parent"]);
  assertEquals(visible(projection.internalWorkers[0].console), [
    "child-answer",
    "alert:a",
  ]);
});

Deno.test("snapshot retains warning Alerts but reconciles durable run errors", () => {
  let projection = emit(
    emptyConsoleProjection(),
    alert("warn", { kind: "start" }),
  );
  projection = emit(projection, {
    ...alert("error", { kind: "start" }),
    level: "error",
    message: "failed",
  });
  projection = apply(
    projection,
    snapshot([{
      kind: "run_error",
      entry_id: "failure",
      timestamp: 300,
      provenance: "legacy_unknown",
      derived_from: [],
      message: "failed",
      failure: "compaction",
    }]),
  );
  assertEquals(
    projectVisibleConsole(projection).lines.map((line) =>
      line.alert?.id ?? line.title
    ),
    ["warn", "Compaction failed"],
  );
});
