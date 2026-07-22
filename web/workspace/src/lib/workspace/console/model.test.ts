import type { Event } from "$lib/generated/protocol";
import {
  createConsoleProjector,
  projectConsole,
  segmentsToText,
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
          output: "/repo\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11\nline12",
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
  assert(line.body.includes('"first": "one"'), "request preview should be shown");
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
  assert(line.body.includes("Grep — 6 matches"), "Grep summary should be shown");
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
    "Tool execution failed: Invalid argument: path is outside allowed scope: /home/hare/.yoi/workdirs/working-directories/0019f5bce74f1000000/root/main/ghq.local/github/openai/codex";
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

Deno.test("projectConsole shows compact progress as a status block", () => {
  const projection = projectConsole([
    {
      eventId: "compact-1",
      event: { event: "compact_start" } satisfies Event,
    },
  ]);

  assertEquals(projection.lines.length, 1);
  assertEquals(projection.lines[0].id, "status-compact");
  assertEquals(projection.lines[0].kind, "status");
  assertEquals(projection.lines[0].body, "Compacting…");
  assertEquals(projection.lines[0].streaming, true);

  const completed = projectConsole([
    {
      eventId: "compact-1",
      event: { event: "compact_start" } satisfies Event,
    },
    {
      eventId: "compact-2",
      event: {
        event: "compact_done",
        data: { new_segment_id: "00000000-0000-0000-0000-000000000001" },
      } satisfies Event,
    },
  ]);

  assertEquals(completed.lines.length, 1);
  assertEquals(completed.lines[0].id, "status-compact");
  assertEquals(completed.lines[0].body, "Compacted.");
  assertEquals(completed.lines[0].streaming, false);
});

Deno.test("createConsoleProjector updates only compact status block", () => {
  const projector = createConsoleProjector();
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
      event: { event: "compact_start" } satisfies Event,
    },
  ]);
  const userLine = projection.lines[0];
  const compactLine = projection.lines[1];

  projection = projector.append([
    {
      eventId: "compact-identity-3",
      event: {
        event: "compact_done",
        data: { new_segment_id: "00000000-0000-0000-0000-000000000001" },
      } satisfies Event,
    },
  ]);

  assert(
    projection.lines[0] === userLine,
    "unrelated message line should keep object identity",
  );
  assert(
    projection.lines[1] !== compactLine,
    "compact status line should update object identity",
  );
  assertEquals(projection.lines[1].body, "Compacted.");
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

Deno.test("projectConsole keeps protocol lifecycle events out of the console surface", () => {
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
        data: { item: { kind: "note", content: "internal" } },
      } satisfies Event,
    },
  ]);

  assertEquals(projection.lines, []);
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
    projection.lines.map((line) => `${line.kind}:${line.body}:${line.streaming}`),
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
