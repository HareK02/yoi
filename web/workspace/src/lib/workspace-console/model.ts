import type {
  Event as ProtocolEvent,
  InFlightBlock,
  Segment,
} from "$lib/generated/protocol";
import type { WorkerTranscriptItem } from "$lib/workspace-sidebar/types";
import { workspaceRoute } from "$lib/workspace-api/http";

export type ConsoleLineKind =
  | "user"
  | "assistant"
  | "thinking"
  | "tool"
  | "status"
  | "error"
  | "usage"
  | "in_flight"
  | "system";

export type ConsoleLine = {
  id: string;
  kind: ConsoleLineKind;
  title: string;
  body: string;
  detail?: string;
  cursor?: string | null;
  source: "initial" | "live";
  streaming?: boolean;
  error?: boolean;
};

export type ConsoleProjection = {
  lines: ConsoleLine[];
  status: string | null;
  usage: string | null;
  lastCursor: string | null;
};

export type WorkerTarget = {
  runtime_id: string;
  worker_id: string;
};

export function workerConsoleHref(target: WorkerTarget, workspaceId: string): string {
  return workspaceRoute(
    workspaceId,
    `/runtimes/${encodeURIComponent(target.runtime_id)}/workers/${
      encodeURIComponent(
        target.worker_id,
      )
    }/console`,
  );
}

export function workerConsolePath(
  workspaceId: string,
  runtimeId: string,
  workerId: string,
): string {
  return workerConsoleHref({ runtime_id: runtimeId, worker_id: workerId }, workspaceId);
}

export function initialConsoleLines(items: WorkerTranscriptItem[]): ConsoleLine[] {
  return items.map((item) => ({
    id: `initial-${item.event_id}-${item.sequence}`,
    kind: initialRoleKind(item.role),
    title: item.role,
    body: item.content,
    source: "initial",
  }));
}

export function projectConsole(
  initialItems: WorkerTranscriptItem[],
  events: Array<{ cursor: string; event: ProtocolEvent }> = [],
): ConsoleProjection {
  return events.reduce(applyProtocolEvent, {
    lines: initialConsoleLines(initialItems),
    status: null,
    usage: null,
    lastCursor: null,
  });
}

export function applyProtocolEvent(
  projection: ConsoleProjection,
  envelope: { cursor: string; event: ProtocolEvent },
): ConsoleProjection {
  const next: ConsoleProjection = {
    lines: [...projection.lines],
    status: projection.status,
    usage: projection.usage,
    lastCursor: envelope.cursor,
  };
  const event = envelope.event;

  switch (event.event) {
    case "user_message":
      next.lines.push(
        line(
          envelope.cursor,
          "user",
          "user message",
          segmentsToText(event.data.segments),
        ),
      );
      break;
    case "system_item":
      // System items are protocol/internal context, not console output.
      break;
    case "text_delta":
      appendStreaming(
        next,
        envelope.cursor,
        "assistant",
        "assistant streaming",
        event.data.text,
      );
      break;
    case "text_done":
      finalizeStreaming(
        next,
        "assistant",
        envelope.cursor,
        "assistant",
        event.data.text,
      );
      break;
    case "thinking_start":
      next.lines.push(
        line(envelope.cursor, "thinking", "thinking", "", undefined, true),
      );
      break;
    case "thinking_delta":
      appendStreaming(
        next,
        envelope.cursor,
        "thinking",
        "thinking",
        event.data.text,
      );
      break;
    case "thinking_done":
      finalizeStreaming(
        next,
        "thinking",
        envelope.cursor,
        "thinking",
        event.data.text,
      );
      break;
    case "tool_call_start":
      next.lines.push(
        line(
          envelope.cursor,
          "tool",
          `tool call · ${event.data.name}`,
          `id: ${event.data.id}`,
          undefined,
          true,
        ),
      );
      break;
    case "tool_call_args_delta":
      appendToolArgs(next, envelope.cursor, event.data.id, event.data.json);
      break;
    case "tool_call_done":
      next.lines.push(
        line(
          envelope.cursor,
          "tool",
          `tool call done · ${event.data.name}`,
          event.data.arguments,
          `id: ${event.data.id}`,
        ),
      );
      break;
    case "tool_result":
      next.lines.push(
        line(
          envelope.cursor,
          "tool",
          event.data.is_error ? "tool result error" : "tool result",
          event.data.output ?? event.data.summary,
          `id: ${event.data.id} · ${event.data.summary}`,
          false,
          event.data.is_error,
        ),
      );
      break;
    case "usage":
      next.usage = usageText(event.data);
      break;
    case "error":
      next.lines.push(
        line(
          envelope.cursor,
          "error",
          `error · ${event.data.code}`,
          event.data.message,
          undefined,
          false,
          true,
        ),
      );
      break;
    case "snapshot":
      next.status = event.data.status;
      for (const block of event.data.in_flight?.blocks ?? []) {
        next.lines.push(inFlightLine(envelope.cursor, block));
      }
      break;
    case "status":
      next.status = event.data.status;
      break;
    case "invoke_start":
    case "turn_start":
    case "turn_end":
    case "llm_call_start":
    case "llm_call_end":
    case "llm_retry":
    case "llm_continuation":
    case "run_end":
    case "alert":
    case "memory_worker":
    case "segment_rotated":
    case "completions":
    case "rewind_targets":
    case "rewind_applied":
    case "workers_listed":
    case "worker_restored":
    case "peer_registered":
    case "compact_start":
    case "compact_done":
      // These are protocol/status/control events. TUI Console does not append
      // them to the conversation surface; browser Console should not either.
      break;
    case "compact_failed":
      next.lines.push(
        line(
          envelope.cursor,
          "error",
          "compact failed",
          event.data.error,
          undefined,
          false,
          true,
        ),
      );
      break;
    case "shutdown":
      next.status = "shutdown";
      break;
  }

  return next;
}

export function segmentsToText(segments: Segment[]): string {
  return segments
    .map((segment) => {
      switch (segment.kind) {
        case "text":
          return segment.content;
        case "paste":
          return segment.content ||
            `[paste ${segment.id}: ${segment.chars} chars / ${segment.lines} lines]`;
        case "file_ref":
          return `@file ${segment.path}`;
        case "knowledge_ref":
          return `@knowledge ${segment.slug}`;
        case "workflow_invoke":
          return `/${segment.slug}`;
        case "unknown":
          return "[unknown segment]";
      }
    })
    .join("\n");
}

function initialRoleKind(role: string): ConsoleLineKind {
  if (role === "user" || role === "assistant" || role === "system") {
    return role;
  }
  return "system";
}

function line(
  cursor: string,
  kind: ConsoleLineKind,
  title: string,
  body: string,
  detail?: string,
  streaming = false,
  error = false,
): ConsoleLine {
  return {
    id: `event-${cursor}-${kind}-${slugify(title)}-${body.length}`,
    kind,
    title,
    body,
    detail,
    cursor,
    source: "live",
    streaming,
    error,
  };
}

function appendStreaming(
  projection: ConsoleProjection,
  cursor: string,
  kind: "assistant" | "thinking",
  title: string,
  delta: string,
): void {
  const existing = [...projection.lines].reverse().find((item) =>
    item.kind === kind && item.streaming
  );
  if (existing) {
    existing.body += delta;
    existing.cursor = cursor;
    return;
  }
  projection.lines.push(line(cursor, kind, title, delta, undefined, true));
}

function finalizeStreaming(
  projection: ConsoleProjection,
  kind: "assistant" | "thinking",
  cursor: string,
  title: string,
  body: string,
): void {
  const existing = [...projection.lines].reverse().find((item) =>
    item.kind === kind && item.streaming
  );
  if (existing) {
    existing.body = body || existing.body;
    existing.streaming = false;
    existing.title = title;
    existing.cursor = cursor;
    return;
  }
  projection.lines.push(line(cursor, kind, title, body));
}

function appendToolArgs(
  projection: ConsoleProjection,
  cursor: string,
  id: string,
  delta: string,
): void {
  const existing = [...projection.lines]
    .reverse()
    .find((item) =>
      item.kind === "tool" && item.streaming && item.body.includes(`id: ${id}`)
    );
  if (existing) {
    existing.body += delta;
    existing.cursor = cursor;
    return;
  }
  projection.lines.push(
    line(
      cursor,
      "tool",
      "tool call args",
      `id: ${id}\n${delta}`,
      undefined,
      true,
    ),
  );
}

function usageText(
  data: {
    input_tokens: number | null;
    output_tokens: number | null;
    cache_read_input_tokens?: number | null;
  },
): string {
  return `input ${data.input_tokens ?? "unknown"} · output ${
    data.output_tokens ?? "unknown"
  } · cache ${data.cache_read_input_tokens ?? "unknown"}`;
}

function inFlightLine(cursor: string, block: InFlightBlock): ConsoleLine {
  switch (block.kind) {
    case "text":
      return line(
        cursor,
        "in_flight",
        "in-flight assistant text",
        block.text,
        undefined,
        !block.finished,
      );
    case "thinking":
      return line(
        cursor,
        "in_flight",
        "in-flight thinking",
        block.text,
        undefined,
        !block.finished,
      );
    case "tool_call":
      return line(
        cursor,
        "in_flight",
        `in-flight tool · ${block.name}`,
        block.args,
        `${block.id} · ${block.state ?? "pending"}`,
        block.state !== "done",
      );
  }
}

function slugify(value: string): string {
  return value.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(
    /^-|-$/g,
    "",
  ) || "event";
}

function jsonPreview(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2) ?? "null";
  } catch {
    return String(value);
  }
}
