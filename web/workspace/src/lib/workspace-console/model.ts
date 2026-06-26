import type {
  Event as ProtocolEvent,
  InFlightBlock,
  Segment,
} from "$lib/generated/protocol";
import type { WorkerTranscriptItem } from "$lib/workspace-sidebar/types";

export type ConsoleLineKind =
  | "user"
  | "assistant"
  | "thinking"
  | "tool"
  | "status"
  | "error"
  | "usage"
  | "snapshot"
  | "in_flight"
  | "system";

export type ConsoleLine = {
  id: string;
  kind: ConsoleLineKind;
  title: string;
  body: string;
  detail?: string;
  cursor?: string | null;
  source: "transcript" | "event";
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

export function workerConsoleHref(target: WorkerTarget): string {
  return `/runtimes/${encodeURIComponent(target.runtime_id)}/workers/${
    encodeURIComponent(
      target.worker_id,
    )
  }/console`;
}

export function workerConsolePath(runtimeId: string, workerId: string): string {
  return workerConsoleHref({ runtime_id: runtimeId, worker_id: workerId });
}

export function transcriptLines(items: WorkerTranscriptItem[]): ConsoleLine[] {
  return items.map((item) => ({
    id: `transcript-${item.event_id}-${item.sequence}`,
    kind: transcriptRoleKind(item.role),
    title: `${item.role} · transcript #${item.sequence}`,
    body: item.content,
    detail: `event ${item.event_id}`,
    source: "transcript",
  }));
}

export function projectConsole(
  transcript: WorkerTranscriptItem[],
  events: Array<{ cursor: string; event: ProtocolEvent }> = [],
): ConsoleProjection {
  return events.reduce(applyProtocolEvent, {
    lines: transcriptLines(transcript),
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
      next.lines.push(
        line(
          envelope.cursor,
          "system",
          "system item",
          jsonPreview(event.data.item),
        ),
      );
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
      next.lines.push(line(envelope.cursor, "usage", "usage", next.usage));
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
      next.lines.push(
        line(
          envelope.cursor,
          "snapshot",
          `snapshot · ${event.data.status}`,
          `${event.data.entries.length} entries · ${event.data.greeting.provider} / ${event.data.greeting.model}`,
          `${event.data.greeting.worker_name} · context ${event.data.greeting.context_tokens}/${event.data.greeting.context_window}`,
        ),
      );
      for (const block of event.data.in_flight?.blocks ?? []) {
        next.lines.push(inFlightLine(envelope.cursor, block));
      }
      break;
    case "status":
      next.status = event.data.status;
      next.lines.push(
        line(envelope.cursor, "status", "status", event.data.status),
      );
      break;
    case "invoke_start":
      next.lines.push(
        line(envelope.cursor, "status", "invoke start", event.data.kind),
      );
      break;
    case "turn_start":
      next.lines.push(
        line(
          envelope.cursor,
          "status",
          "turn start",
          `turn ${event.data.turn}`,
        ),
      );
      break;
    case "turn_end":
      next.lines.push(
        line(
          envelope.cursor,
          "status",
          "turn end",
          `turn ${event.data.turn} · ${event.data.result}`,
        ),
      );
      break;
    case "llm_call_start":
      next.lines.push(
        line(
          envelope.cursor,
          "status",
          "llm call start",
          `call ${event.data.llm_call}`,
        ),
      );
      break;
    case "llm_call_end":
      next.lines.push(
        line(
          envelope.cursor,
          "status",
          "llm call end",
          `call ${event.data.llm_call}`,
        ),
      );
      break;
    case "llm_retry":
      next.lines.push(
        line(
          envelope.cursor,
          "status",
          "llm retry",
          `${event.data.error} · attempt ${event.data.failed_attempt}/${event.data.max_attempts}`,
        ),
      );
      break;
    case "llm_continuation":
      next.lines.push(
        line(
          envelope.cursor,
          "status",
          "llm continuation",
          `${event.data.reason} · attempt ${event.data.attempt}/${event.data.max_attempts}`,
        ),
      );
      break;
    case "run_end":
      next.lines.push(
        line(envelope.cursor, "status", "run end", event.data.result),
      );
      break;
    case "alert":
      next.lines.push(
        line(
          envelope.cursor,
          "status",
          `alert · ${event.data.level}`,
          event.data.message,
        ),
      );
      break;
    case "memory_worker":
      next.lines.push(
        line(envelope.cursor, "status", "memory worker", event.data.message),
      );
      break;
    case "segment_rotated":
      next.lines.push(
        line(
          envelope.cursor,
          "status",
          "segment rotated",
          jsonPreview(event.data.entry),
        ),
      );
      break;
    case "completions":
      next.lines.push(
        line(
          envelope.cursor,
          "status",
          "completions",
          `${event.data.kind} · ${event.data.entries.length} entries`,
        ),
      );
      break;
    case "rewind_targets":
      next.lines.push(
        line(
          envelope.cursor,
          "status",
          "rewind targets",
          `${event.data.targets.length} targets · head ${event.data.head_entries}`,
        ),
      );
      break;
    case "rewind_applied":
      next.lines.push(
        line(
          envelope.cursor,
          "status",
          "rewind applied",
          `${event.data.summary.discarded_entries} discarded · ${event.data.summary.truncated_to_entries} retained`,
        ),
      );
      break;
    case "workers_listed":
      next.lines.push(
        line(
          envelope.cursor,
          "status",
          "workers listed",
          jsonPreview(event.data.workers),
        ),
      );
      break;
    case "worker_restored":
      next.lines.push(
        line(
          envelope.cursor,
          "status",
          "worker restored",
          jsonPreview(event.data.result),
        ),
      );
      break;
    case "peer_registered":
      next.lines.push(
        line(
          envelope.cursor,
          "status",
          "peer registered",
          jsonPreview(event.data.result),
        ),
      );
      break;
    case "compact_start":
      next.lines.push(
        line(envelope.cursor, "status", "compact start", "compaction started"),
      );
      break;
    case "compact_done":
      next.lines.push(
        line(
          envelope.cursor,
          "status",
          "compact done",
          event.data.new_segment_id,
        ),
      );
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
      next.lines.push(
        line(envelope.cursor, "status", "shutdown", "worker shut down"),
      );
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

function transcriptRoleKind(role: string): ConsoleLineKind {
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
    source: "event",
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
