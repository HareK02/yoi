import type { Event } from "$lib/generated/protocol";
import {
  type ConsoleEventInput,
  type ConsoleLine,
  consoleWorkerViews,
  createConsoleProjector,
  isConsoleProjectionEvent,
  projectConsole,
  projectConsoleLines,
  projectOverviewLines,
  resolveConsoleViewScrollTop,
  resolveConsoleWorkerView,
  segmentsToText,
  selectConsoleTimelineLines,
  workerConsoleHref,
} from "./model.ts";

declare const Deno: {
  test(name: string, fn: () => void): void;
};

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) {
    throw new Error(message);
  }
}

function assertEquals<T>(actual: T, expected: T): void {
  const actualJson = JSON.stringify(actual);
  const expectedJson = JSON.stringify(expected);
  if (actualJson !== expectedJson) {
    throw new Error(`Expected ${expectedJson}, got ${actualJson}`);
  }
}

function consoleLine(id: string, kind: ConsoleLine["kind"]): ConsoleLine {
  return {
    id,
    kind,
    title: kind,
    body: id,
    source: "event",
  };
}

function snapshotEvent(cwd: string, entries: unknown[] = []): Event {
  return {
    event: "snapshot",
    data: {
      entries,
      greeting: {
        worker_name: "Worker",
        cwd,
        provider: "provider",
        model: "model",
        scope_summary: "bounded",
        tools: [],
        context_window: 100,
        context_tokens: 20,
      },
      status: "idle",
      in_flight: { blocks: [] },
    },
  };
}

Deno.test("console routing projects live errors but not completion replies", () => {
  const errorEvent = {
    event: "error",
    data: { code: "provider_error", message: "provider unavailable" },
  } satisfies Event;
  const completionEvent = {
    event: "completions",
    data: { kind: "file", entries: [] },
  } satisfies Event;

  assert(
    isConsoleProjectionEvent(errorEvent),
    "live errors must reach the timeline projector",
  );
  assert(
    !isConsoleProjectionEvent(completionEvent),
    "completion replies should remain control-only events",
  );
});

Deno.test("snapshot replaces a live error with one durable run_errored row", () => {
  const projector = createConsoleProjector();
  let projection = projector.append([
    {
      eventId: "live-error",
      event: {
        event: "error",
        data: { code: "provider_error", message: "provider unavailable" },
      } satisfies Event,
    },
    {
      eventId: "idle-after-error",
      event: { event: "status", data: { status: "idle" } } satisfies Event,
    },
  ]);

  assertEquals(projection.status, "idle");
  const liveErrors = projection.lines.filter((line) => line.kind === "error");
  assertEquals(liveErrors.length, 1);
  assertEquals(liveErrors[0].title, "error · provider_error");
  assertEquals(liveErrors[0].body, "provider unavailable");

  projection = projector.append([{
    eventId: "reconnected-snapshot",
    event: snapshotEvent("/repo", [{
      kind: "run_errored",
      ts: 3,
      interrupted: false,
      message: "provider unavailable",
    }]),
  }]);

  const errors = projection.lines.filter((line) => line.kind === "error");
  assertEquals(errors.length, 1);
  assertEquals(errors[0].title, "Run error");
  assertEquals(errors[0].body, "provider unavailable");
  assertEquals(errors[0].error, true);
});

Deno.test("segment rotation retains a live error beside the real SegmentStart history", () => {
  const projector = createConsoleProjector();
  const projection = projector.append([
    {
      eventId: "live-error",
      event: {
        event: "error",
        data: { code: "provider_error", message: "provider unavailable" },
      } satisfies Event,
    },
    {
      eventId: "segment-rotated",
      event: {
        event: "segment_rotated",
        data: {
          entry: {
            kind: "segment_start",
            ts: 5,
            session_id: "session-1",
            system_prompt: null,
            config: {},
            history: [{
              kind: "message",
              role: "user",
              content: [{ kind: "text", text: "retained conversation" }],
            }],
          },
        },
      } satisfies Event,
    },
  ]);

  const errors = projection.lines.filter((line) => line.kind === "error");
  assertEquals(errors.length, 1);
  assertEquals(errors[0].title, "error · provider_error");
  assertEquals(errors[0].body, "provider unavailable");
  assert(
    projection.lines.some((line) => line.body === "retained conversation"),
    "SegmentStart history should still seed the rotated projection",
  );
});

Deno.test("workerConsoleHref encodes runtime and worker target authority", () => {
  assert(
    workerConsoleHref({
      runtime_id: "local runtime",
      worker_id: "worker/one",
    }, "workspace-1") ===
      "/w/workspace-1/runtimes/local%20runtime/workers/worker%2Fone/console",
    "href should contain encoded runtime_id and worker_id segments",
  );
});

Deno.test("projectConsole projects visible protocol rows", () => {
  const projection = projectConsole([
    {
      eventId: "10",
      event: {
        event: "user_message",
        data: { segments: [{ kind: "text", content: "input" }] },
      } satisfies Event,
    },
    {
      eventId: "11",
      event: {
        event: "text_delta",
        data: { text: "stream" },
      } satisfies Event,
    },
    {
      eventId: "12",
      event: {
        event: "thinking_done",
        data: { text: "reasoning" },
      } satisfies Event,
    },
    {
      eventId: "13",
      event: {
        event: "tool_result",
        data: {
          id: "tool-1",
          summary: "read file",
          output: "content",
          is_error: false,
        },
      } satisfies Event,
    },
    {
      eventId: "14",
      event: {
        event: "usage",
        data: { input_tokens: 12, output_tokens: 5 },
      } satisfies Event,
    },
    {
      eventId: "15",
      event: {
        event: "error",
        data: { code: "invalid_request", message: "bad frame" },
      } satisfies Event,
    },
  ]);

  assert(
    projection.lines.some((line) =>
      line.source === "event" && line.kind === "user"
    ),
    "user protocol row expected",
  );
  assert(
    projection.lines.some((line) =>
      line.source === "event" && line.kind === "assistant"
    ),
    "assistant protocol row expected",
  );
  assert(
    projection.lines.some((line) => line.kind === "thinking"),
    "thinking event row expected",
  );
  assert(
    projection.lines.some((line) => line.kind === "tool"),
    "tool event row expected",
  );
  assert(
    !projection.lines.some((line) => line.kind === "usage"),
    "usage should update the summary without rendering a console row",
  );
  assert(
    projection.lines.some((line) => line.kind === "error" && line.error),
    "error event row expected",
  );
  assert(
    projection.usage === "input 12 · output 5 · cache unknown",
    "usage summary should be retained",
  );
});

Deno.test("projectConsole groups tool call lifecycle into one Call block", () => {
  const projection = projectConsole([
    {
      eventId: "40",
      event: {
        event: "tool_call_start",
        data: { id: "call-1", name: "Bash" },
      } satisfies Event,
    },
    {
      eventId: "41",
      event: {
        event: "tool_call_args_delta",
        data: { id: "call-1", json: '{"command":"pw' },
      } satisfies Event,
    },
    {
      eventId: "42",
      event: {
        event: "tool_call_args_delta",
        data: { id: "call-1", json: 'd"}' },
      } satisfies Event,
    },
    {
      eventId: "43",
      event: {
        event: "tool_call_done",
        data: { id: "call-1", name: "Bash", arguments: '{"command":"pwd"}' },
      } satisfies Event,
    },
    {
      eventId: "44",
      event: {
        event: "tool_result",
        data: {
          id: "call-1",
          summary: "command completed",
          output:
            "/repo\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11\nline12",
          is_error: false,
        },
      } satisfies Event,
    },
  ]);

  const toolLines = projection.lines.filter((line) => line.kind === "tool");
  assertEquals(toolLines.length, 1);
  assertEquals(toolLines[0].title, "Call · Bash");
  assert(
    !toolLines[0].streaming,
    "completed tool call should not remain streaming",
  );
  assert(
    toolLines[0].body.includes("$ pwd"),
    "Bash command should be summarized",
  );
  assert(
    toolLines[0].body.includes("/repo"),
    "tool result should be folded into the Call block",
  );
  assert(
    toolLines[0].body.includes("line9"),
    "Bash result preview should include the ninth output line",
  );
  assert(
    !toolLines[0].body.includes("line10") &&
      !toolLines[0].body.includes("line12"),
    "Bash result preview should be capped at ten display lines",
  );
  assert(
    toolLines[0].body.includes("… +3 more lines"),
    "Bash result preview should show omitted output count",
  );
  assert(
    toolLines[0].detail?.includes("id: call-1"),
    "call id should remain in detail",
  );
});

Deno.test("projectConsole streams distinct Bash stdout and stderr through terminal status", () => {
  const projection = projectConsole([
    {
      eventId: "command-tool",
      event: {
        event: "tool_call_done",
        data: {
          id: "bash-stream",
          name: "Bash",
          arguments: JSON.stringify({ command: "long-command" }),
        },
      } satisfies Event,
    },
    {
      eventId: "command-started",
      event: {
        event: "command",
        data: {
          event: {
            kind: "started",
            command_id: "command-1",
            tool_call_id: "bash-stream",
            observed_at_ms: 1000,
          },
        },
      } satisfies Event,
    },
    {
      eventId: "command-stdout",
      event: {
        event: "command",
        data: {
          event: {
            kind: "output",
            command_id: "command-1",
            stream: "stdout",
            start_offset: 0,
            end_offset: 6,
            content: "ready\n",
            observed_at_ms: 1100,
          },
        },
      } satisfies Event,
    },
    {
      eventId: "command-stderr",
      event: {
        event: "command",
        data: {
          event: {
            kind: "output",
            command_id: "command-1",
            stream: "stderr",
            start_offset: 0,
            end_offset: 5,
            content: "warn\n",
            observed_at_ms: 1200,
          },
        },
      } satisfies Event,
    },
    {
      eventId: "command-terminal",
      event: {
        event: "command",
        data: {
          event: {
            kind: "terminal",
            command_id: "command-1",
            status: "failed",
            exit_code: 7,
            stdout_end_offset: 6,
            stderr_end_offset: 5,
            observed_at_ms: 1300,
          },
        },
      } satisfies Event,
    },
  ]);

  const [line] = projection.lines.filter((line) => line.kind === "tool");
  assert(line.body.includes("Bash — failed (exit 7)"), line.body);
  assert(!line.body.includes("elapsed"), line.body);
  assert(!line.body.includes("stdout:"), line.body);
  assert(line.body.includes("ready\n"), line.body);
  assert(line.body.includes("stderr:\nwarn\n"), line.body);
  assert(line.detail?.includes("command: elapsed 300ms"), line.detail ?? "");
  assertEquals(line.streaming, false);
  assertEquals(line.error, true);
});

Deno.test("snapshot restores bounded in-flight Bash command output", () => {
  const snapshot = snapshotEvent("/repo");
  if (snapshot.event !== "snapshot") throw new Error("snapshot fixture expected");
  snapshot.data.status = "running";
  snapshot.data.in_flight = {
    blocks: [{
      kind: "tool_call",
      id: "bash-snapshot",
      name: "Bash",
      args: JSON.stringify({ command: "slow" }),
      state: "done",
    }],
    commands: [{
      command_id: "command-2",
      tool_call_id: "bash-snapshot",
      status: "running",
      started_at_ms: 1000,
      observed_at_ms: 1250,
      last_output_at_ms: 1200,
      stdout: {
        start_offset: 1024,
        end_offset: 1031,
        content: "tail\n",
        truncated: true,
      },
      stderr: { start_offset: 0, end_offset: 0, content: "", truncated: false },
      exit_code: null,
    }],
  };

  const projection = projectConsole([{ eventId: "snapshot-command", event: snapshot }]);
  const [line] = projection.lines.filter((line) => line.kind === "tool");
  assert(line.body.includes("Bash — running…"), line.body);
  assert(!line.body.includes("elapsed"), line.body);
  assert(!line.body.includes("stdout:"), line.body);
  assert(line.body.includes("[… earlier stdout omitted]\ntail\n"), line.body);
  assert(
    line.detail?.includes("command: elapsed 250ms · last output at +200ms"),
    line.detail ?? "",
  );
  assertEquals(line.streaming, true);
});

Deno.test("projectConsole caps default tool request and result previews", () => {
  const projection = projectConsole([
    {
      eventId: "70",
      event: {
        event: "tool_call_done",
        data: {
          id: "custom-1",
          name: "CustomTool",
          arguments: JSON.stringify({
            first: "one",
            second: "two",
            third: "three",
            fourth: "four",
          }),
        },
      } satisfies Event,
    },
    {
      eventId: "71",
      event: {
        event: "tool_result",
        data: {
          id: "custom-1",
          summary: "custom completed",
          output: "out1\nout2\nout3\nout4\nout5",
          is_error: false,
        },
      } satisfies Event,
    },
  ]);

  const [line] = projection.lines.filter((line) => line.kind === "tool");
  assertEquals(line.title, "Call · CustomTool");
  assertEquals(line.body.split("\n").length, 7);
  assert(line.body.includes("CustomTool — done"), "tool state should be shown");
  assert(
    line.body.includes('"first": "one"'),
    "request preview should be shown",
  );
  assert(line.body.includes("out1"), "result preview should be shown");
  assert(!line.body.includes("third"), "request preview should be capped");
  assert(!line.body.includes("out3"), "result preview should be capped");
  assert(line.body.includes("… +"), "overflow marker should be shown");
});

Deno.test("projectConsole shows Grep query and caps result preview to five entries", () => {
  const projection = projectConsole([
    {
      eventId: "72",
      event: {
        event: "tool_call_done",
        data: {
          id: "grep-1",
          name: "Grep",
          arguments: JSON.stringify({ pattern: "needle", path: "/repo" }),
        },
      } satisfies Event,
    },
    {
      eventId: "73",
      event: {
        event: "tool_result",
        data: {
          id: "grep-1",
          summary: "6 matches",
          output: "hit1\nhit2\nhit3\nhit4\nhit5\nhit6",
          is_error: false,
        },
      } satisfies Event,
    },
  ]);

  const [line] = projection.lines.filter((line) => line.kind === "tool");
  assertEquals(line.title, "Call · Grep");
  assert(
    line.body.includes("Grep — 6 matches"),
    "Grep summary should be shown",
  );
  assert(line.body.includes("query: needle"), "Grep query should be shown");
  assert(line.body.includes("hit1"), "first result should be shown");
  assert(line.body.includes("hit5"), "fifth result should be shown");
  assert(!line.body.includes("hit6"), "sixth result should be capped");
  assert(
    line.body.includes("… +1 more results"),
    "overflow marker should be shown",
  );
});

Deno.test("projectConsole keeps Grep error detail in the body", () => {
  const message =
    "Tool execution failed: Invalid argument: path is outside allowed scope: /home/hare/.yoi/workdirs/0019f5bce74f1000000/checkout/ghq.local/github/openai/codex";
  const projection = projectConsole([
    {
      eventId: "74",
      event: {
        event: "tool_call_done",
        data: {
          id: "grep-error-1",
          name: "Grep",
          arguments: JSON.stringify({ pattern: "needle", path: "/outside" }),
        },
      } satisfies Event,
    },
    {
      eventId: "75",
      event: {
        event: "tool_result",
        data: {
          id: "grep-error-1",
          summary: message,
          output: message,
          is_error: true,
        },
      } satisfies Event,
    },
  ]);

  const [line] = projection.lines.filter((line) => line.kind === "tool");
  assertEquals(line.title, "Call · Grep");
  assert(
    line.body.includes("Grep — Failed"),
    "error suffix should stay short",
  );
  assert(
    line.body.includes(message),
    "error detail should remain visible in the body",
  );
  assert(
    !line.body.includes(`Grep — ${message}`),
    "error detail should not be repeated in the suffix",
  );
});

Deno.test("projectConsole renders alert events", () => {
  const projection = projectConsole([
    {
      eventId: "alert-1",
      event: {
        event: "alert",
        data: {
          level: "warn",
          source: "compactor",
          message: "manual compaction skipped",
          timestamp_ms: 1,
        },
      } satisfies Event,
    },
    {
      eventId: "alert-2",
      event: {
        event: "alert",
        data: {
          level: "error",
          source: "engine",
          message: "provider failed",
          timestamp_ms: 2,
        },
      } satisfies Event,
    },
  ]);

  assertEquals(projection.lines.length, 2);
  assertEquals(projection.lines[0].kind, "status");
  assertEquals(projection.lines[0].title, "Alert · compactor");
  assertEquals(projection.lines[0].body, "manual compaction skipped");
  assertEquals(projection.lines[0].error, false);
  assertEquals(projection.lines[1].kind, "error");
  assertEquals(projection.lines[1].title, "Alert · engine");
  assertEquals(projection.lines[1].body, "provider failed");
  assertEquals(projection.lines[1].error, true);
});

Deno.test("projectConsole upserts compaction lifecycle by stable id", () => {
  const running = {
    schema_version: 2,
    compaction_id: "compaction-1",
    revision: 1,
    internal_worker: null,
    state: "running",
    started_at_ms: 1_000,
    ended_at_ms: null,
    summary: null,
    error: null,
    new_segment_id: null,
  } as const;
  const projection = projectConsole([
    {
      eventId: "compact-1",
      event: { event: "compact_start", data: { lifecycle: running } } satisfies Event,
    },
  ]);

  assertEquals(projection.lines.length, 1);
  assertEquals(projection.lines[0].id, "compaction-compaction-1");
  assertEquals(projection.lines[0].compaction?.state, "running");
  assertEquals(projection.lines[0].streaming, true);

  const completed = projectConsole([
    {
      eventId: "compact-1",
      event: { event: "compact_start", data: { lifecycle: running } } satisfies Event,
    },
    {
      eventId: "compact-2",
      event: {
        event: "compact_done",
        data: {
          lifecycle: {
            ...running,
            revision: 2,
            state: "done",
            ended_at_ms: 4_000,
            summary: "accepted summary",
            new_segment_id: "00000000-0000-0000-0000-000000000001",
          },
        },
      } satisfies Event,
    },
  ]);

  assertEquals(completed.lines.length, 1);
  assertEquals(completed.lines[0].id, "compaction-compaction-1");
  assertEquals(completed.lines[0].compaction?.state, "done");
  assertEquals(completed.lines[0].compaction?.summary, "accepted summary");
  assertEquals(completed.lines[0].streaming, false);
});

Deno.test("compaction service activity stays nested in one lifecycle item", () => {
  const worker = {
    session_id: "compactor-session",
    name: "Compaction",
    parent_session_id: "parent-session",
    kind: { service: { kind: "compaction" } },
  } as const;
  const lifecycle = {
    schema_version: 2,
    compaction_id: "compaction-nested",
    revision: 2,
    internal_worker: worker,
    state: "running",
    started_at_ms: 1_000,
    ended_at_ms: null,
    summary: null,
    error: null,
    new_segment_id: null,
  } as const;
  const projection = projectConsole([
    {
      eventId: "compaction-start",
      event: { event: "compact_start", data: { lifecycle } } satisfies Event,
    },
    {
      eventId: "compaction-tool",
      event: {
        event: "internal_worker",
        data: {
          worker,
          revision: 1,
          event: {
            event: "tool_call_start",
            data: { id: "call-1", name: "write_summary" },
          },
        },
      } satisfies Event,
    },
    {
      eventId: "compaction-tool-done",
      event: {
        event: "internal_worker",
        data: {
          worker,
          revision: 2,
          event: {
            event: "tool_call_done",
            data: {
              id: "call-1",
              name: "write_summary",
              arguments: JSON.stringify({ text: "draft candidate" }),
            },
          },
        },
      } satisfies Event,
    },
  ]);

  assertEquals(projection.lines.length, 1);
  assertEquals(projection.lines[0].id, "compaction-compaction-nested");
  assertEquals(projection.lines[0].compaction?.activity, ["write_summary — running"]);
  assertEquals(projection.lines[0].compaction?.candidate, "draft candidate");
  assert(
    consoleWorkerViews(projection).length === 1,
    "service is not a selectable SubWorker pane",
  );
});

Deno.test("snapshot normalizes orphaned running compaction to interrupted", () => {
  const projection = projectConsole([{
    eventId: "snapshot",
    observedAtMs: 9_000,
    event: snapshotEvent("/repo", [{
      kind: "extension",
      ts: 1,
      domain: "yoi.compaction",
      payload: {
        schema_version: 2,
        compaction_id: "compaction-orphaned",
        revision: 2,
        internal_worker: {
          session_id: "missing-service",
          name: "Compaction",
          parent_session_id: "parent",
          kind: { service: { kind: "compaction" } },
        },
        state: "running",
        started_at_ms: 1_000,
      },
    }]),
  }]);

  assertEquals(projection.lines.length, 1);
  assertEquals(projection.lines[0].compaction?.state, "interrupted");
  assertEquals(projection.lines[0].compaction?.endedAtMs, 9_000);
  assertEquals(projection.lines[0].streaming, false);
});

Deno.test("createConsoleProjector ignores stale compaction revisions", () => {
  const projector = createConsoleProjector();
  const base = {
    schema_version: 2,
    compaction_id: "compaction-identity",
    revision: 1,
    internal_worker: null,
    state: "running",
    started_at_ms: 1_000,
    ended_at_ms: null,
    summary: null,
    error: null,
    new_segment_id: null,
  } as const;
  let projection = projector.append([
    {
      eventId: "compact-identity-1",
      event: {
        event: "user_message",
        data: { segments: [{ kind: "text", content: "hello" }] },
      } satisfies Event,
    },
    {
      eventId: "compact-identity-2",
      event: { event: "compact_start", data: { lifecycle: base } } satisfies Event,
    },
  ]);
  const userLine = projection.lines[0];

  projection = projector.append([
    {
      eventId: "compact-identity-3",
      event: {
        event: "compact_done",
        data: {
          lifecycle: {
            ...base,
            revision: 2,
            state: "done",
            ended_at_ms: 2_000,
            summary: "accepted",
            new_segment_id: "00000000-0000-0000-0000-000000000001",
          },
        },
      } satisfies Event,
    },
    {
      eventId: "compact-identity-stale",
      event: { event: "compact_start", data: { lifecycle: base } } satisfies Event,
    },
  ]);

  assert(projection.lines[0] === userLine, "unrelated line retains identity");
  assertEquals(projection.lines[1].compaction?.state, "done");
  assertEquals(projection.lines[1].compaction?.summary, "accepted");
});

Deno.test("projectConsole keeps streaming tool call updates in the same Call block", () => {
  const projection = projectConsole([
    {
      eventId: "45",
      event: {
        event: "tool_call_start",
        data: { id: "call-2", name: "Read" },
      } satisfies Event,
    },
    {
      eventId: "46",
      event: {
        event: "tool_call_args_delta",
        data: { id: "call-2", json: '{"file_path":"/tmp/a.md"}' },
      } satisfies Event,
    },
  ]);

  const toolLines = projection.lines.filter((line) => line.kind === "tool");
  assertEquals(toolLines.length, 1);
  assertEquals(toolLines[0].title, "Call · Read");
  assert(toolLines[0].streaming, "streaming tool call should remain streaming");
  assert(
    toolLines[0].body.includes("/tmp/a.md") &&
      toolLines[0].body.includes("Read — reading"),
    "Read call should render aggregate progress and path without content",
  );
});

Deno.test("createConsoleProjector replaces only the updated protocol block", () => {
  const projector = createConsoleProjector();
  let projection = projector.append([
    {
      eventId: "identity-1",
      event: {
        event: "user_message",
        data: { segments: [{ kind: "text", content: "hello" }] },
      } satisfies Event,
    },
    {
      eventId: "identity-2",
      event: {
        event: "tool_call_start",
        data: { id: "grep-a", name: "Grep" },
      } satisfies Event,
    },
    {
      eventId: "identity-3",
      event: {
        event: "tool_call_start",
        data: { id: "grep-b", name: "Grep" },
      } satisfies Event,
    },
  ]);

  const userLine = projection.lines[0];
  const grepA = projection.lines[1];
  const grepB = projection.lines[2];

  projection = projector.append([
    {
      eventId: "identity-4",
      event: {
        event: "tool_call_args_delta",
        data: { id: "grep-a", json: '{"pattern":"needle"}' },
      } satisfies Event,
    },
  ]);

  assert(
    projection.lines[0] === userLine,
    "unrelated message line should keep object identity",
  );
  assert(
    projection.lines[1] !== grepA,
    "updated tool line should get a new object identity",
  );
  assert(
    projection.lines[2] === grepB,
    "parallel unrelated tool line should keep object identity",
  );

  const updatedGrepA = projection.lines[1];
  projection = projector.append([
    {
      eventId: "identity-5",
      event: {
        event: "tool_result",
        data: {
          id: "grep-b",
          summary: "done",
          output: "hit",
          is_error: false,
        },
      } satisfies Event,
    },
  ]);

  assert(
    projection.lines[1] === updatedGrepA,
    "previously updated but now unrelated tool line should keep identity",
  );
  assert(
    projection.lines[2] !== grepB,
    "completed parallel tool line should get a new object identity",
  );
});

Deno.test("projectConsole aggregates Read calls without showing file content", () => {
  const projection = projectConsole([
    {
      eventId: "50",
      event: {
        event: "tool_call_done",
        data: {
          id: "read-1",
          name: "Read",
          arguments: '{"file_path":"/tmp/a.md"}',
        },
      } satisfies Event,
    },
    {
      eventId: "51",
      event: {
        event: "tool_result",
        data: {
          id: "read-1",
          summary: "Read 2 lines",
          output: "secret file content\nsecond line\n",
          is_error: false,
        },
      } satisfies Event,
    },
    {
      eventId: "52",
      event: {
        event: "tool_call_done",
        data: {
          id: "read-2",
          name: "Read",
          arguments: '{"file_path":"/tmp/b.md"}',
        },
      } satisfies Event,
    },
    {
      eventId: "53",
      event: {
        event: "tool_result",
        data: {
          id: "read-2",
          summary: "Read 1 line",
          output: "another content\n",
          is_error: false,
        },
      } satisfies Event,
    },
  ]);

  const toolLines = projection.lines.filter((line) => line.kind === "tool");
  assertEquals(toolLines.length, 1);
  assertEquals(toolLines[0].title, "Call · Read");
  assert(
    toolLines[0].body.includes("Read — 2 files read"),
    "aggregate count should be shown",
  );
  assert(
    toolLines[0].body.includes("/tmp/a.md"),
    "first path should be listed",
  );
  assert(
    toolLines[0].body.includes("/tmp/b.md"),
    "second path should be listed",
  );
  assert(
    !toolLines[0].body.includes("secret file content") &&
      !toolLines[0].body.includes("another content"),
    "Read aggregate should not display file contents",
  );
  const overview = projectOverviewLines(projection.lines);
  assertEquals(overview.length, 1);
  assertEquals(overview[0].body, "2 files read");
});

Deno.test("projectConsole renders Edit calls with structured diff lines", () => {
  const projection = projectConsole([
    {
      eventId: "60",
      event: {
        event: "tool_call_done",
        data: {
          id: "edit-1",
          name: "Edit",
          arguments: JSON.stringify({
            file_path: "/tmp/a.md",
            old_string: "one\ntwo\nthree",
            new_string: "one\nTWO\nthree\nfour",
          }),
        },
      } satisfies Event,
    },
    {
      eventId: "61",
      event: {
        event: "tool_result",
        data: {
          id: "edit-1",
          summary: "edited",
          output: "ok",
          is_error: false,
        },
      } satisfies Event,
    },
  ]);

  const [line] = projection.lines.filter((line) => line.kind === "tool");
  assertEquals(line.title, "Call · Edit");
  assert(line.body.includes("diff: -1 +2"), "diff summary should be shown");
  assertEquals(line.diff?.map((row) => row.kind), [
    "context",
    "remove",
    "add",
    "context",
    "add",
  ]);
});

Deno.test("projectConsole preserves in-progress assistant protocol stream", () => {
  const projection = projectConsole([
    {
      eventId: "13",
      event: { event: "text_delta", data: { text: "new" } } satisfies Event,
    },
  ]);

  assert(
    projection.lines.some((line) =>
      line.source === "event" && line.kind === "assistant" &&
      line.body === "new" && line.streaming
    ),
    "in-progress assistant stream should remain visible",
  );
});

Deno.test("projectConsole hides lifecycle events and renders system items", () => {
  const projection = projectConsole([
    {
      eventId: "30",
      event: { event: "status", data: { status: "running" } } satisfies Event,
    },
    {
      eventId: "31",
      event: { event: "llm_call_end", data: { llm_call: 0 } } satisfies Event,
    },
    {
      eventId: "32",
      event: {
        event: "turn_end",
        data: { turn: 0, result: "finished" },
      } satisfies Event,
    },
    {
      eventId: "33",
      event: { event: "run_end", data: { result: "finished" } } satisfies Event,
    },
    {
      eventId: "34",
      event: {
        event: "system_item",
        data: {
          item: {
            kind: "notification",
            message: "Ticket queued",
            body: "Reread Ticket 00001KZ6TSGG5 before acting.",
          },
        },
      } satisfies Event,
    },
  ]);

  assertEquals(projection.lines.length, 2);
  assertEquals(projection.lines[0].kind, "run_stats");
  assertEquals(projection.lines[0].body, "0s ・0 reqs ↑0/↓0");
  assertEquals(projection.lines[1].kind, "system");
  assertEquals(projection.lines[1].title, "System · notification");
  assertEquals(
    projection.lines[1].body,
    "Reread Ticket 00001KZ6TSGG5 before acting.",
  );
  assertEquals(projection.status, "running");
});

Deno.test("projectConsole renders snapshot entries and in-flight output", () => {
  const projection = projectConsole([
    {
      eventId: "20",
      event: {
        event: "snapshot",
        data: {
          entries: [
            {
              kind: "segment_start",
              ts: 1,
              session_id: "00000000-0000-0000-0000-000000000001",
              system_prompt: null,
              config: {},
              history: [
                {
                  kind: "message",
                  role: "user",
                  content: [{ kind: "text", text: "seed user" }],
                },
              ],
            },
            {
              kind: "user_input",
              ts: 2,
              segments: [{ kind: "text", content: "new user" }],
            },
            {
              kind: "assistant_item",
              ts: 3,
              item: {
                kind: "message",
                role: "assistant",
                content: [{ kind: "text", text: "assistant reply" }],
              },
            },
            {
              kind: "assistant_item",
              ts: 4,
              item: {
                kind: "tool_call",
                call_id: "read-1",
                name: "Read",
                arguments: JSON.stringify({ file_path: "/tmp/a.md" }),
              },
            },
            {
              kind: "tool_result",
              ts: 5,
              item: {
                kind: "tool_result",
                call_id: "read-1",
                summary: "read 3 lines",
                content: "hidden file contents",
                is_error: false,
              },
            },
            {
              kind: "extension",
              ts: 6,
              domain: "yoi.compaction",
              payload: {
                kind: "compaction_block",
                schema_version: 1,
                block_id: "compact",
                state: "running",
                message: "Compacting…",
              },
            },
          ],
          greeting: {
            worker_name: "Worker",
            cwd: "/repo",
            provider: "provider",
            model: "model",
            scope_summary: "bounded",
            tools: ["Read"],
            context_window: 100,
            context_tokens: 20,
          },
          status: "running",
          in_flight: {
            blocks: [
              { kind: "text", text: "partial" },
            ],
          },
        },
      } satisfies Event,
    },
  ]);

  assertEquals(projection.status, "running");
  assertEquals(
    projection.lines.map((line) =>
      `${line.kind}:${line.body}:${line.streaming}`
    ),
    [
      "user:seed user:false",
      "user:new user:false",
      "assistant:assistant reply:false",
      "tool:Read — 1 file read\n  /tmp/a.md:false",
      "status:Compacting…:true",
      "in_flight:partial:true",
    ],
  );
});

Deno.test("projectConsole restores system items from snapshot entries", () => {
  const projection = projectConsole([{
    eventId: "system-snapshot",
    event: {
      event: "snapshot",
      data: {
        entries: [{
          kind: "system_item",
          ts: 1,
          item: {
            kind: "notification",
            message: "Worker completed",
            body: "Child Worker coder-1 completed.",
          },
        }],
        greeting: {
          worker_name: "Worker",
          cwd: "/repo",
          provider: "provider",
          model: "model",
          scope_summary: "bounded",
          tools: [],
          context_window: 100,
          context_tokens: 20,
        },
        status: "idle",
      },
    } satisfies Event,
  }]);

  assertEquals(projection.lines.length, 1);
  assertEquals(projection.lines[0].kind, "system");
  assertEquals(projection.lines[0].title, "System · notification");
  assertEquals(projection.lines[0].body, "Child Worker coder-1 completed.");
});

Deno.test("projectConsole reseeds visible rows from segment rotation", () => {
  const projection = projectConsole([
    {
      eventId: "rotation-before",
      event: {
        event: "user_message",
        data: { segments: [{ kind: "text", content: "before rotation" }] },
      } satisfies Event,
    },
    {
      eventId: "rotation-event",
      event: {
        event: "segment_rotated",
        data: {
          entry: {
            kind: "segment_start",
            ts: 10,
            session_id: "00000000-0000-0000-0000-000000000001",
            system_prompt: null,
            config: {},
            history: [
              {
                kind: "message",
                role: "user",
                content: [{ kind: "text", text: "after rotation seed" }],
              },
            ],
          },
        },
      } satisfies Event,
    },
  ]);

  assertEquals(
    projection.lines.map((line) => `${line.kind}:${line.body}`),
    ["user:after rotation seed"],
  );
});

Deno.test("selectConsoleTimelineLines keeps all users and only last assistant per turn", () => {
  const items = [
    consoleLine("u1", "user"),
    consoleLine("a1", "assistant"),
    consoleLine("tool1", "tool"),
    consoleLine("a2", "assistant"),
    consoleLine("u2", "user"),
    consoleLine("a3", "assistant"),
    consoleLine("a4", "assistant"),
  ];

  assertEquals(
    selectConsoleTimelineLines(items).map(({ item, index }) =>
      `${index}:${item.id}`
    ),
    ["0:u1", "3:a2", "4:u2", "6:a4"],
  );
});

Deno.test("projectConsole relativizes known tool path displays from snapshot cwd", () => {
  const projection = projectConsole([
    { eventId: "cwd-0", event: snapshotEvent("/repo") },
    {
      eventId: "cwd-1",
      event: {
        event: "tool_call_done",
        data: {
          id: "read-rel",
          name: "Read",
          arguments: JSON.stringify({ file_path: "/repo/src/main.rs" }),
        },
      } satisfies Event,
    },
    {
      eventId: "cwd-2",
      event: {
        event: "tool_result",
        data: {
          id: "read-rel",
          summary: "Read 4 line(s) [1..4] of 20 from /repo/src/main.rs",
          is_error: false,
        },
      } satisfies Event,
    },
    {
      eventId: "cwd-3",
      event: {
        event: "tool_call_done",
        data: {
          id: "write-rel",
          name: "Write",
          arguments: JSON.stringify({
            file_path: "/repo/out.txt",
            content: "ok",
          }),
        },
      } satisfies Event,
    },
    {
      eventId: "cwd-4",
      event: {
        event: "tool_result",
        data: {
          id: "write-rel",
          summary: "Wrote /repo/out.txt",
          output: "Wrote /repo/out.txt",
          is_error: false,
        },
      } satisfies Event,
    },
    {
      eventId: "cwd-5",
      event: {
        event: "tool_call_done",
        data: {
          id: "edit-rel",
          name: "Edit",
          arguments: JSON.stringify({
            file_path: "/repo/src/main.rs",
            old_string: "a",
            new_string: "b",
          }),
        },
      } satisfies Event,
    },
    {
      eventId: "cwd-6",
      event: {
        event: "tool_result",
        data: {
          id: "edit-rel",
          summary: "Edited /repo/src/main.rs (1 replacement)",
          output: "Edited /repo/src/main.rs (1 replacement)",
          is_error: false,
        },
      } satisfies Event,
    },
    {
      eventId: "cwd-7",
      event: {
        event: "tool_call_done",
        data: {
          id: "glob-rel",
          name: "Glob",
          arguments: JSON.stringify({ path: "/repo", pattern: "**/*.rs" }),
        },
      } satisfies Event,
    },
    {
      eventId: "cwd-8",
      event: {
        event: "tool_result",
        data: {
          id: "glob-rel",
          summary: "Found 2 file(s) matching **/*.rs",
          output:
            "Found 2 file(s) matching **/*.rs\n/repo/src/main.rs\n/outside/lib.rs",
          is_error: false,
        },
      } satisfies Event,
    },
    {
      eventId: "cwd-9",
      event: {
        event: "tool_call_done",
        data: {
          id: "grep-rel",
          name: "Grep",
          arguments: JSON.stringify({ pattern: "needle", path: "/repo" }),
        },
      } satisfies Event,
    },
    {
      eventId: "cwd-10",
      event: {
        event: "tool_result",
        data: {
          id: "grep-rel",
          summary: "2 matching line(s) in 2 file(s)",
          output: "/repo/src/main.rs:12:needle\n/outside/lib.rs:1:needle",
          is_error: false,
        },
      } satisfies Event,
    },
  ]);

  const bodies = projection.lines.filter((line) => line.kind === "tool").map((
    line,
  ) => line.body);
  assertEquals(bodies[0], "Read — 1 file read\n  src/main.rs");
  assert(
    projection.lines[0].detail?.includes("from src/main.rs"),
    "Read summary detail path should be relative",
  );
  assert(
    bodies.some((body) =>
      body.includes("Write — out.txt") && body.includes("Wrote out.txt")
    ),
    "Write header and known result path should be relative",
  );
  assert(
    bodies.some((body) =>
      body.includes("Edit — src/main.rs") && body.includes("Edited src/main.rs")
    ),
    "Edit header and known result path should be relative",
  );
  assert(
    bodies.some((body) =>
      body.includes("src/main.rs") && body.includes("/outside/lib.rs")
    ),
    "Glob should relativize cwd paths and keep outside paths absolute",
  );
  assert(
    bodies.some((body) =>
      body.includes("src/main.rs:12:needle") &&
      body.includes("/outside/lib.rs:1:needle")
    ),
    "Grep should relativize line-start cwd paths and keep outside paths absolute",
  );
});

Deno.test("projectConsole mirrors live TaskCreate and TaskUpdate calls", () => {
  const projection = projectConsole([
    {
      eventId: "task-create",
      event: {
        event: "tool_call_done",
        data: {
          id: "task-create-1",
          name: "TaskCreate",
          arguments: JSON.stringify({
            subject: "Port Tasks",
            description: "Render active Worker tasks",
          }),
        },
      } satisfies Event,
    },
    {
      eventId: "task-update",
      event: {
        event: "tool_call_done",
        data: {
          id: "task-update-1",
          name: "TaskUpdate",
          arguments: JSON.stringify({ taskid: 1, status: "inprogress" }),
        },
      } satisfies Event,
    },
  ]);

  assertEquals(projection.tasks, [{
    taskid: 1,
    status: "inprogress",
    subject: "Port Tasks",
    description: "Render active Worker tasks",
  }]);
});

Deno.test("Internal Worker output stays separate and revision-fenced", () => {
  const worker = {
    session_id: "child-session",
    name: "research",
    parent_session_id: "parent-session",
    kind: "sub_worker" as const,
  };
  const projector = createConsoleProjector();
  let projection = projector.append([{
    eventId: "1",
    event: {
      event: "internal_worker",
      data: {
        worker,
        revision: 2,
        event: { event: "text_done", data: { text: "child output" } },
      },
    },
  }]);
  assertEquals(projection.lines, []);
  assertEquals(projection.internalWorkers.length, 1);
  assertEquals(
    projection.internalWorkers[0].console.lines[0].body,
    "child output",
  );

  projection = projector.append([{
    eventId: "2",
    event: {
      event: "internal_worker",
      data: {
        worker,
        revision: 1,
        event: { event: "text_done", data: { text: "stale" } },
      },
    },
  }]);
  assertEquals(projection.internalWorkers[0].console.lines.length, 1);

  const views = consoleWorkerViews(projection);
  assertEquals(views.map((view) => [view.sessionId, view.label]), [
    [null, "main"],
    ["child-session", "research"],
  ]);
  assertEquals(
    resolveConsoleWorkerView(projection, "child-session").console.lines[0].body,
    "child output",
  );
  assertEquals(resolveConsoleWorkerView(projection, "missing").sessionId, null);
});

Deno.test("console Worker views expose only direct Internal Workers", () => {
  const projector = createConsoleProjector();
  projector.append([{
    eventId: "nested",
    event: {
      event: "internal_worker",
      data: {
        worker: {
          session_id: "child-session",
          name: "research",
          parent_session_id: "parent-session",
          kind: "sub_worker",
        },
        revision: 1,
        event: {
          event: "internal_worker",
          data: {
            worker: {
              session_id: "grandchild-session",
              name: "nested",
              parent_session_id: "child-session",
              kind: "sub_worker",
            },
            revision: 1,
            event: { event: "status", data: { status: "running" } },
          },
        },
      },
    },
  }]);

  projector.append([{
    eventId: "peer",
    event: {
      event: "internal_worker",
      data: {
        worker: {
          session_id: "peer-other",
          name: "research",
          parent_session_id: "parent-session",
          kind: "sub_worker",
        },
        revision: 1,
        event: { event: "status", data: { status: "idle" } },
      },
    },
  }]);

  const projection = projector.snapshot();
  const views = consoleWorkerViews(projection);
  assertEquals(views.map((view) => view.sessionId), [
    null,
    "child-session",
    "peer-other",
  ]);
  assertEquals(views[1].label, "research · ession");
  assertEquals(views[2].label, "research · -other");
  assertEquals(
    resolveConsoleWorkerView(projection, "grandchild-session").sessionId,
    null,
  );
});

Deno.test("console Worker view scroll restores manual offsets and auto-follow", () => {
  assertEquals(resolveConsoleViewScrollTop(undefined, 1000, 200), 1000);
  assertEquals(
    resolveConsoleViewScrollTop({ top: 100, autoFollow: true }, 1000, 200),
    1000,
  );
  assertEquals(
    resolveConsoleViewScrollTop({ top: 300, autoFollow: false }, 1000, 200),
    300,
  );
  assertEquals(
    resolveConsoleViewScrollTop({ top: 900, autoFollow: false }, 1000, 200),
    800,
  );
});

Deno.test("parent snapshot authoritatively replaces Internal Worker projections", () => {
  const event = snapshotEvent("/repo");
  if (event.event !== "snapshot") throw new Error("snapshot fixture expected");
  event.data.internal_workers = [{
    worker: {
      session_id: "replacement",
      name: "replacement",
      parent_session_id: "parent-session",
      kind: "sub_worker",
    },
    revision: 4,
    entries: [{
      kind: "assistant_item",
      ts: 1,
      item: {
        kind: "tool_call",
        call_id: "committed-call",
        name: "Read",
        arguments: JSON.stringify({ file_path: "/repo/a.md" }),
      },
    }, {
      kind: "tool_result",
      ts: 2,
      item: {
        kind: "tool_result",
        call_id: "committed-call",
        summary: "read file",
        content: "content",
        is_error: false,
      },
    }],
    status: "idle",
    in_flight: {
      blocks: [{
        kind: "tool_call",
        id: "committed-call",
        name: "Read",
        args: JSON.stringify({ file_path: "/repo/a.md" }),
        state: "done",
      }],
    },
    internal_workers: [],
  }];
  const projector = createConsoleProjector();
  projector.append([{
    eventId: "old",
    event: {
      event: "internal_worker",
      data: {
        worker: {
          session_id: "old",
          name: "old",
          parent_session_id: "parent-session",
          kind: "sub_worker",
        },
        revision: 1,
        event: { event: "status", data: { status: "running" } },
      },
    },
  }]);
  const projection = projector.append([{ eventId: "snapshot", event }]);
  assertEquals(
    projection.internalWorkers.map((worker) => worker.worker.session_id),
    [
      "replacement",
    ],
  );
  const childLines = projection.internalWorkers[0].console.lines;
  assertEquals(childLines.length, 1);
  assertEquals(new Set(childLines.map((line) => line.id)).size, 1);
  assertEquals(childLines[0].kind, "tool");
});

Deno.test("terminal Internal Worker removal drops descendants and fences late events", () => {
  const worker = {
    session_id: "child-session",
    name: "child",
    parent_session_id: "parent-session",
    kind: "sub_worker" as const,
  };
  const nestedWorker = {
    session_id: "grandchild-session",
    name: "grandchild",
    parent_session_id: "child-session",
    kind: "sub_worker" as const,
  };
  const projector = createConsoleProjector();
  let projection = projector.append([{
    eventId: "child",
    event: {
      event: "internal_worker",
      data: {
        worker,
        revision: 2,
        event: {
          event: "internal_worker",
          data: {
            worker: nestedWorker,
            revision: 1,
            event: { event: "text_done", data: { text: "nested" } },
          },
        },
      },
    },
  }]);
  assertEquals(projection.internalWorkers.length, 1);
  assertEquals(
    projection.internalWorkers[0].console.internalWorkers.length,
    1,
  );

  projection = projector.append([{
    eventId: "removed",
    event: {
      event: "internal_worker_removed",
      data: { worker, revision: 3 },
    },
  }, {
    eventId: "late",
    event: {
      event: "internal_worker",
      data: {
        worker,
        revision: 4,
        event: { event: "text_done", data: { text: "must stay removed" } },
      },
    },
  }]);
  assertEquals(projection.internalWorkers, []);

  const snapshot = snapshotEvent("/repo");
  projection = projector.append([{ eventId: "snapshot", event: snapshot }]);
  assertEquals(projection.internalWorkers, []);
  assertEquals(projection.removedInternalWorkers, {});
});

Deno.test("stale Internal Worker removal cannot discard a newer projection", () => {
  const worker = {
    session_id: "child-session",
    name: "child",
    parent_session_id: "parent-session",
    kind: "sub_worker" as const,
  };
  const projector = createConsoleProjector();
  projector.append([{
    eventId: "current",
    event: {
      event: "internal_worker",
      data: {
        worker,
        revision: 4,
        event: { event: "text_done", data: { text: "current" } },
      },
    },
  }]);

  const projection = projector.append([{
    eventId: "stale-removal",
    event: {
      event: "internal_worker_removed",
      data: { worker, revision: 3 },
    },
  }]);
  assertEquals(projection.internalWorkers.length, 1);
  assertEquals(projection.internalWorkers[0].revision, 4);
});

Deno.test("snapshot restores TaskStore state from system history", () => {
  const taskSnapshot =
    `[Session TaskStore snapshot]\n\n\`\`\`json\n{\n  "tasks": [{"taskid": 3, "status": "pending", "subject": "Restored", "description": "From compaction"}]\n}\n\`\`\``;
  const event = snapshotEvent("/repo");
  if (event.event !== "snapshot") throw new Error("snapshot fixture expected");
  event.data.entries = [{
    kind: "segment_start",
    ts: 1,
    session_id: "00000000-0000-0000-0000-000000000001",
    system_prompt: null,
    config: {},
    history: [{
      kind: "message",
      role: "system",
      content: [{ kind: "text", text: taskSnapshot }],
    }],
  }];

  const projection = projectConsole([{ eventId: "task-snapshot", event }]);
  assertEquals(projection.tasks, [{
    taskid: 3,
    status: "pending",
    subject: "Restored",
    description: "From compaction",
  }]);
  assertEquals(projection.taskNextId, 4);
});

Deno.test("overview hides typed task reminders after restoring TaskStore state", () => {
  const body =
    `[Session TaskStore snapshot]\n\n\`\`\`json\n{\n  "tasks": [{"taskid": 8, "status": "inprogress", "subject": "Visible in Tasks", "description": "Hidden in overview"}]\n}\n\`\`\``;
  const projection = projectConsole([{
    eventId: "task-reminder",
    event: {
      event: "system_item",
      data: {
        item: { kind: "task_reminder", body },
      },
    },
  }]);

  assertEquals(projection.tasks[0]?.taskid, 8);
  assertEquals(projection.lines[0]?.systemItemKind, "task_reminder");
  assertEquals(projectConsoleLines(projection.lines, "overview"), []);
  const normal = projectConsoleLines(projection.lines, "normal");
  assertEquals(normal.length, 1);
  assertEquals(normal[0].kind, "task_reminder");
  assertEquals(
    normal[0].body,
    "task reminder: [Session TaskStore snapshot]",
  );
});

Deno.test("overview hides thinking and aggregates uninterrupted tool activity", () => {
  const toolLine = (
    id: string,
    name: string,
    diff?: ConsoleLine["diff"],
  ): ConsoleLine => ({
    id,
    kind: "tool",
    title: `Call · ${name}`,
    body: name,
    source: "event",
    diff,
    toolCall: {
      id,
      name,
      argsStream: "",
      state: "done",
    },
  });

  const overview = projectOverviewLines([
    consoleLine("user", "user"),
    consoleLine("assistant-before", "assistant"),
    consoleLine("thought-before-tools", "thinking"),
    toolLine("read-a", "Read"),
    consoleLine("thought-between-tools", "thinking"),
    toolLine("read-b", "Read"),
    toolLine("bash-a", "Bash"),
    consoleLine("assistant-after-tools", "assistant"),
    toolLine("edit-a", "Edit", [
      { kind: "remove", oldNumber: 1, content: "old" },
      { kind: "add", newNumber: 1, content: "new" },
      { kind: "add", newNumber: 2, content: "next" },
    ]),
  ]);

  assertEquals(overview.map((line) => line.kind), [
    "user",
    "assistant",
    "activity",
    "assistant",
    "activity",
  ]);
  assertEquals(overview[2].body, "2 files read・ran 1 command");
  assertEquals(overview[4].body, "edited +2/-1");
});

Deno.test("overview hides in-flight thinking and keeps tool failures visible", () => {
  const overview = projectOverviewLines([
    {
      ...consoleLine("thinking-in-flight", "in_flight"),
      title: "in-flight thinking",
    },
    {
      ...consoleLine("failed-read", "tool"),
      error: true,
      toolCall: {
        id: "failed-read",
        name: "Read",
        argsStream: "",
        state: "error",
        isError: true,
      },
    },
  ]);

  assertEquals(overview.length, 1);
  assertEquals(overview[0].kind, "activity");
  assertEquals(overview[0].body, "1 file read\n1 failed");
  assertEquals(overview[0].error, true);
});

Deno.test("RunEnd appends TUI-compatible request and token stats", () => {
  const events: ConsoleEventInput[] = [
    {
      eventId: "invoke",
      observedAtMs: 1_000,
      event: { event: "invoke_start", data: { kind: "user_send" } },
    },
    ...Array.from({ length: 5 }, (_, index) => ({
      eventId: `turn-${index}`,
      observedAtMs: 1_010 + index,
      event: { event: "turn_start", data: { turn: index + 1 } } as Event,
    })),
    {
      eventId: "usage",
      observedAtMs: 1_020,
      event: {
        event: "usage",
        data: {
          input_tokens: 60_000,
          cache_read_input_tokens: 3_500,
          output_tokens: 1_200,
        },
      },
    },
    {
      eventId: "run-end",
      observedAtMs: 621_000,
      event: { event: "run_end", data: { result: "finished" } },
    },
  ];

  const projection = projectConsole(events);
  const stats = projection.lines.filter((line) => line.kind === "run_stats");
  assertEquals(stats.length, 1);
  assertEquals(stats[0].body, "10m20s ・5 reqs ↑56.5k/↓1.2k");
  assertEquals(
    projectConsoleLines(projection.lines, "overview").at(-1)?.kind,
    "run_stats",
  );
  assertEquals(
    projectConsoleLines(projection.lines, "normal").at(-1)?.kind,
    "run_stats",
  );
});

Deno.test("new invoke resets stats before the next RunEnd", () => {
  const projector = createConsoleProjector();
  projector.append([
    {
      eventId: "first-invoke",
      event: { event: "invoke_start", data: { kind: "user_send" } },
    },
    {
      eventId: "first-turn",
      event: { event: "turn_start", data: { turn: 1 } },
    },
    {
      eventId: "first-usage",
      event: {
        event: "usage",
        data: { input_tokens: 1_000, output_tokens: 100 },
      },
    },
    {
      eventId: "first-end",
      event: { event: "run_end", data: { result: "finished" } },
    },
  ]);
  const projection = projector.append([
    {
      eventId: "second-invoke",
      event: { event: "invoke_start", data: { kind: "notify" } },
    },
    {
      eventId: "second-end",
      event: { event: "run_end", data: { result: "finished" } },
    },
  ]);

  assertEquals(projection.lines.at(-1)?.body, "0s ・0 reqs ↑0/↓0");
});
