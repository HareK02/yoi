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
      cursor: "10",
      event: {
        event: "user_message",
        data: { segments: [{ kind: "text", content: "input" }] },
      } satisfies Event,
    },
    {
      cursor: "11",
      event: {
        event: "text_delta",
        data: { text: "stream" },
      } satisfies Event,
    },
    {
      cursor: "12",
      event: {
        event: "thinking_done",
        data: { text: "reasoning" },
      } satisfies Event,
    },
    {
      cursor: "13",
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
      cursor: "14",
      event: {
        event: "usage",
        data: { input_tokens: 12, output_tokens: 5 },
      } satisfies Event,
    },
    {
      cursor: "15",
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
      cursor: "40",
      event: {
        event: "tool_call_start",
        data: { id: "call-1", name: "Bash" },
      } satisfies Event,
    },
    {
      cursor: "41",
      event: {
        event: "tool_call_args_delta",
        data: { id: "call-1", json: '{"command":"pw' },
      } satisfies Event,
    },
    {
      cursor: "42",
      event: {
        event: "tool_call_args_delta",
        data: { id: "call-1", json: 'd"}' },
      } satisfies Event,
    },
    {
      cursor: "43",
      event: {
        event: "tool_call_done",
        data: { id: "call-1", name: "Bash", arguments: '{"command":"pwd"}' },
      } satisfies Event,
    },
    {
      cursor: "44",
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
      cursor: "45",
      event: {
        event: "tool_call_start",
        data: { id: "call-2", name: "Read" },
      } satisfies Event,
    },
    {
      cursor: "46",
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
      cursor: "50",
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
      cursor: "51",
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
      cursor: "52",
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
      cursor: "53",
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

Deno.test("projectConsole preserves in-progress assistant protocol stream", () => {
  const projection = projectConsole([
    {
      cursor: "13",
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
      cursor: "30",
      event: { event: "status", data: { status: "running" } } satisfies Event,
    },
    {
      cursor: "31",
      event: { event: "llm_call_end", data: { llm_call: 0 } } satisfies Event,
    },
    {
      cursor: "32",
      event: {
        event: "turn_end",
        data: { turn: 0, result: "finished" },
      } satisfies Event,
    },
    {
      cursor: "33",
      event: { event: "run_end", data: { result: "finished" } } satisfies Event,
    },
    {
      cursor: "34",
      event: {
        event: "system_item",
        data: { item: { kind: "note", content: "internal" } },
      } satisfies Event,
    },
  ]);

  assertEquals(projection.lines, []);
  assertEquals(projection.status, "running");
});

Deno.test("projectConsole uses snapshot for state without rendering it as console output", () => {
  const projection = projectConsole([
    {
      cursor: "20",
      event: {
        event: "snapshot",
        data: {
          entries: [{ role: "user" }],
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
    ["in_flight:partial:true"],
  );
});
