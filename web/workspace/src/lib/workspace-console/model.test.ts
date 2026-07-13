import type { Event } from "$lib/generated/protocol";
import { projectConsole, segmentsToText, workerConsoleHref } from "./model.ts";

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

Deno.test("segmentsToText preserves protocol segment semantics", () => {
  const text = segmentsToText([
    { kind: "text", content: "hello" },
    { kind: "file_ref", path: "/tmp/example.md" },
    { kind: "knowledge_ref", slug: "design-note" },
    { kind: "workflow_invoke", slug: "ticket-review" },
  ]);

  assert(text.includes("hello"), "text segment should render content");
  assert(
    text.includes("@file /tmp/example.md"),
    "file ref should render as a file reference",
  );
  assert(
    text.includes("@knowledge design-note"),
    "knowledge ref should render as a knowledge reference",
  );
  assert(
    text.includes("/ticket-review"),
    "workflow invocation should render as slash command",
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
          output: "/repo\n",
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
    toolLines[0].detail?.includes("id: call-1"),
    "call id should remain in detail",
  );
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
      "in_flight:partial:true",
    ],
  );
});
