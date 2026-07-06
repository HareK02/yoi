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

Deno.test("projectConsole projects initial console output and live visible protocol rows", () => {
  const projection = projectConsole(
    [
      {
        sequence: 1,
        role: "user",
        content: "transcript input",
        event_id: 10,
      },
    ],
    [
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
    ],
  );

  assert(
    projection.lines.some((line) =>
      line.source === "initial" && line.kind === "user"
    ),
    "initial user row expected",
  );
  assert(
    projection.lines.some((line) =>
      line.source === "live" && line.kind === "assistant"
    ),
    "assistant live row expected",
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

Deno.test("projectConsole keeps protocol lifecycle events out of the console surface", () => {
  const projection = projectConsole([], [
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
      event: { event: "turn_end", data: { turn: 0, result: "finished" } } satisfies Event,
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
  const projection = projectConsole([], [
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
              { kind: "text", text: "unfinished answer", finished: false },
              {
                kind: "tool_call",
                id: "call-1",
                name: "Read",
                args: "{}",
                state: "streaming_args",
              },
            ],
          },
        },
      } satisfies Event,
    },
  ]);

  assert(projection.status === "running", "snapshot should update status");
  assert(
    !projection.lines.some((line) => line.title.includes("snapshot")),
    "snapshot should not render as a console row",
  );
  assert(
    projection.lines.filter((line) => line.kind === "in_flight").length === 2,
    "in-flight rows expected",
  );
});
