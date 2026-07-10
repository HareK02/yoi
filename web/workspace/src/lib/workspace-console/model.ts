import type {
  Event as ProtocolEvent,
  InFlightBlock,
  InFlightToolCallState,
  Segment,
} from "$lib/generated/protocol";
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

type ToolCallState =
  | "pending"
  | "streaming_args"
  | "running"
  | "done"
  | "error";

type ToolCallView = {
  id: string;
  name: string;
  argsStream: string;
  arguments?: string;
  state: ToolCallState;
  summary?: string;
  output?: string | null;
  isError?: boolean;
};

export type ConsoleDiffLine = {
  kind: "context" | "add" | "remove";
  oldNumber?: number;
  newNumber?: number;
  content: string;
};

export type ConsoleLine = {
  id: string;
  kind: ConsoleLineKind;
  title: string;
  body: string;
  detail?: string;
  diff?: ConsoleDiffLine[];
  cursor?: string | null;
  source: "event";
  streaming?: boolean;
  error?: boolean;
  toolCall?: ToolCallView;
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

export function workerConsoleHref(
  target: WorkerTarget,
  workspaceId: string,
): string {
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
  return workerConsoleHref(
    { runtime_id: runtimeId, worker_id: workerId },
    workspaceId,
  );
}

export type ConsoleEventInput = { cursor: string; event: ProtocolEvent };

export function projectConsole(
  events: ConsoleEventInput[] = [],
): ConsoleProjection {
  const projection = events.reduce(applyProtocolEvent, {
    lines: [],
    status: null,
    usage: null,
    lastCursor: null,
  });
  return {
    ...projection,
    lines: aggregateReadToolLines(projection.lines),
  };
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
          "User",
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
        line(envelope.cursor, "thinking", "Thinking...", "", undefined, true),
      );
      break;
    case "thinking_delta":
      appendStreaming(
        next,
        envelope.cursor,
        "thinking",
        "Thinking...",
        event.data.text,
      );
      break;
    case "thinking_done":
      finalizeStreaming(
        next,
        "thinking",
        envelope.cursor,
        "Thought",
        event.data.text,
      );
      break;
    case "tool_call_start":
      upsertToolCall(next, envelope.cursor, event.data.id, {
        name: event.data.name,
        state: "pending",
      });
      break;
    case "tool_call_args_delta":
      appendToolArgs(next, envelope.cursor, event.data.id, event.data.json);
      break;
    case "tool_call_done":
      upsertToolCall(next, envelope.cursor, event.data.id, {
        name: event.data.name,
        arguments: event.data.arguments,
        argsStream: event.data.arguments,
        state: "running",
      });
      break;
    case "tool_result":
      attachToolResult(next, envelope.cursor, event.data.id, {
        summary: event.data.summary,
        output: event.data.output,
        isError: event.data.is_error,
      });
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

function upsertToolCall(
  projection: ConsoleProjection,
  cursor: string,
  id: string,
  update: Partial<Omit<ToolCallView, "id">>,
): ConsoleLine {
  let existing = findToolCallLine(projection, id);
  if (!existing) {
    existing = toolLine(cursor, {
      id,
      name: update.name ?? "Tool",
      argsStream: update.argsStream ?? "",
      arguments: update.arguments,
      state: update.state ?? "pending",
      summary: update.summary,
      output: update.output,
      isError: update.isError,
    });
    projection.lines.push(existing);
  } else {
    existing.cursor = cursor;
    existing.toolCall = {
      ...existing.toolCall!,
      ...update,
      id,
      name: update.name ?? existing.toolCall!.name,
    };
  }
  refreshToolLine(existing);
  return existing;
}

function appendToolArgs(
  projection: ConsoleProjection,
  cursor: string,
  id: string,
  delta: string,
): void {
  const existing = upsertToolCall(projection, cursor, id, {
    state: "streaming_args",
  });
  existing.toolCall!.argsStream += delta;
  refreshToolLine(existing);
}

function attachToolResult(
  projection: ConsoleProjection,
  cursor: string,
  id: string,
  result: Pick<ToolCallView, "summary" | "output" | "isError">,
): void {
  const existing = findToolCallLine(projection, id);
  if (!existing) {
    const fallback = toolLine(cursor, {
      id,
      name: "Tool",
      argsStream: "",
      state: result.isError ? "error" : "done",
      ...result,
    });
    fallback.title = result.isError
      ? "Call · Tool result error"
      : "Call · Tool result";
    refreshToolLine(fallback);
    projection.lines.push(fallback);
    return;
  }
  existing.cursor = cursor;
  existing.toolCall = {
    ...existing.toolCall!,
    ...result,
    state: result.isError ? "error" : "done",
  };
  refreshToolLine(existing);
}

function findToolCallLine(
  projection: ConsoleProjection,
  id: string,
): ConsoleLine | undefined {
  return [...projection.lines].reverse().find((item) =>
    item.toolCall?.id === id
  );
}

function toolLine(cursor: string, toolCall: ToolCallView): ConsoleLine {
  const item: ConsoleLine = {
    id: `tool-call-${toolCall.id}`,
    kind: "tool",
    title: `Call · ${toolCall.name}`,
    body: "",
    detail: undefined,
    cursor,
    source: "event",
    streaming: true,
    error: false,
    toolCall,
  };
  refreshToolLine(item);
  return item;
}

function refreshToolLine(item: ConsoleLine): void {
  const toolCall = item.toolCall;
  if (!toolCall) {
    return;
  }
  item.title = item.title.startsWith("Call · Tool result")
    ? item.title
    : `Call · ${toolCall.name}`;
  item.body = renderToolCall(toolCall);
  item.detail = toolCallDetail(toolCall);
  item.diff = toolCall.name === "Edit" ? editDiff(toolCall) : undefined;
  item.streaming = !["done", "error"].includes(toolCall.state);
  item.error = toolCall.state === "error";
}

function renderToolCall(toolCall: ToolCallView): string {
  switch (toolCall.name) {
    case "Read":
      return renderReadTool(toolCall);
    case "Write":
      return renderWriteTool(toolCall);
    case "Edit":
      return renderEditTool(toolCall);
    case "Glob":
    case "Grep":
      return renderSearchTool(toolCall);
    case "Bash":
      return renderBashTool(toolCall);
    default:
      return renderDefaultTool(toolCall);
  }
}

function aggregateReadToolLines(lines: ConsoleLine[]): ConsoleLine[] {
  const result: ConsoleLine[] = [];
  let index = 0;
  while (index < lines.length) {
    const item = lines[index];
    if (item.toolCall?.name !== "Read") {
      result.push(item);
      index += 1;
      continue;
    }

    const group: ConsoleLine[] = [];
    while (index < lines.length && lines[index].toolCall?.name === "Read") {
      group.push(lines[index]);
      index += 1;
    }
    result.push(readAggregateLine(group));
  }
  return result;
}

function readAggregateLine(group: ConsoleLine[]): ConsoleLine {
  const calls = group.map((line) => line.toolCall!).filter(Boolean);
  const count = calls.length;
  const inProgress = calls.some((call) =>
    !["done", "error"].includes(call.state)
  );
  const hasError = calls.some((call) => call.state === "error");
  const paths = calls.map(readPath);
  const visiblePaths = inProgress ? paths.slice(-3) : paths;
  const body = compactLines([
    inProgress
      ? `Read — reading (${count} file${plural(count)}…)`
      : `Read — ${count} file${plural(count)} read`,
    visiblePaths.map((path) => `  ${path}`).join("\n"),
    inProgress && paths.length > visiblePaths.length
      ? `  … (${paths.length - visiblePaths.length} earlier)`
      : undefined,
  ]);
  return {
    id: `tool-read-aggregate-${calls.map((call) => call.id).join("-")}`,
    kind: "tool",
    title: "Call · Read",
    body,
    detail: calls.map(readDetail).join("\n\n"),
    cursor: group.at(-1)?.cursor,
    source: "event",
    streaming: inProgress,
    error: hasError,
  };
}

function readPath(toolCall: ToolCallView): string {
  const args = parsedArgs(toolCall);
  return stringField(args, "file_path") ?? "?";
}

function readDetail(toolCall: ToolCallView): string {
  return compactLines([
    `id: ${toolCall.id}`,
    `state: ${stateSuffix(toolCall.state)}`,
    `path: ${readPath(toolCall)}`,
    toolCall.summary ? `summary: ${toolCall.summary}` : undefined,
  ]);
}

function renderReadTool(toolCall: ToolCallView): string {
  const args = parsedArgs(toolCall);
  const path = stringField(args, "file_path") ?? "?";
  return `Read — ${path} (${stateSuffix(toolCall.state)})`;
}

function renderWriteTool(toolCall: ToolCallView): string {
  const args = parsedArgs(toolCall);
  const path = stringField(args, "file_path") ?? "?";
  const content = stringField(args, "content");
  return compactLines([
    `Write — ${path} (${stateSuffix(toolCall.state)})`,
    cappedSection(content, 5),
    resultText(toolCall),
  ]);
}

function renderEditTool(toolCall: ToolCallView): string {
  const args = parsedArgs(toolCall);
  const path = stringField(args, "file_path") ?? "?";
  const diff = editDiff(toolCall) ?? [];
  const removes = diff.filter((line) => line.kind === "remove").length;
  const adds = diff.filter((line) => line.kind === "add").length;
  return compactLines([
    `Edit — ${path} (${stateSuffix(toolCall.state)})`,
    diff.length > 0 ? `diff: -${removes} +${adds}` : undefined,
    resultText(toolCall),
  ]);
}

function editDiff(toolCall: ToolCallView): ConsoleDiffLine[] | undefined {
  const args = parsedArgs(toolCall);
  const oldString = stringField(args, "old_string");
  const newString = stringField(args, "new_string");
  if (oldString === undefined && newString === undefined) {
    return undefined;
  }
  return diffLines(oldString ?? "", newString ?? "");
}

function diffLines(oldText: string, newText: string): ConsoleDiffLine[] {
  const oldLines = oldText.split(/\r?\n/);
  const newLines = newText.split(/\r?\n/);
  const table = lcsTable(oldLines, newLines);
  const rows: ConsoleDiffLine[] = [];
  let oldIndex = 0;
  let newIndex = 0;
  while (oldIndex < oldLines.length && newIndex < newLines.length) {
    if (oldLines[oldIndex] === newLines[newIndex]) {
      rows.push({
        kind: "context",
        oldNumber: oldIndex + 1,
        newNumber: newIndex + 1,
        content: oldLines[oldIndex],
      });
      oldIndex += 1;
      newIndex += 1;
    } else if (
      table[oldIndex + 1]?.[newIndex] >= table[oldIndex]?.[newIndex + 1]
    ) {
      rows.push({
        kind: "remove",
        oldNumber: oldIndex + 1,
        content: oldLines[oldIndex],
      });
      oldIndex += 1;
    } else {
      rows.push({
        kind: "add",
        newNumber: newIndex + 1,
        content: newLines[newIndex],
      });
      newIndex += 1;
    }
  }
  while (oldIndex < oldLines.length) {
    rows.push({
      kind: "remove",
      oldNumber: oldIndex + 1,
      content: oldLines[oldIndex],
    });
    oldIndex += 1;
  }
  while (newIndex < newLines.length) {
    rows.push({
      kind: "add",
      newNumber: newIndex + 1,
      content: newLines[newIndex],
    });
    newIndex += 1;
  }
  return rows;
}

function lcsTable(oldLines: string[], newLines: string[]): number[][] {
  const rows = Array.from(
    { length: oldLines.length + 1 },
    () => Array(newLines.length + 1).fill(0),
  );
  for (let oldIndex = oldLines.length - 1; oldIndex >= 0; oldIndex -= 1) {
    for (let newIndex = newLines.length - 1; newIndex >= 0; newIndex -= 1) {
      rows[oldIndex][newIndex] = oldLines[oldIndex] === newLines[newIndex]
        ? rows[oldIndex + 1][newIndex + 1] + 1
        : Math.max(rows[oldIndex + 1][newIndex], rows[oldIndex][newIndex + 1]);
    }
  }
  return rows;
}

function renderSearchTool(toolCall: ToolCallView): string {
  const summary = toolCall.summary?.trim();
  return compactLines([
    `${toolCall.name} — ${
      summary ? firstLine(summary) : stateSuffix(toolCall.state)
    }`,
    resultText(toolCall),
  ]);
}

function renderBashTool(toolCall: ToolCallView): string {
  const args = parsedArgs(toolCall);
  const command = stringField(args, "command");
  return compactLines([
    `Bash — ${stateSuffix(toolCall.state)}`,
    command ? `$ ${command}` : argsText(toolCall),
    resultText(toolCall),
  ]);
}

function renderDefaultTool(toolCall: ToolCallView): string {
  return compactLines([
    `${toolCall.name} — ${stateSuffix(toolCall.state)}`,
    argsText(toolCall),
    resultText(toolCall),
  ]);
}

function toolCallDetail(toolCall: ToolCallView): string {
  return compactLines([
    `id: ${toolCall.id}`,
    `state: ${stateSuffix(toolCall.state)}`,
    toolCall.summary ? `summary: ${toolCall.summary}` : undefined,
    argsText(toolCall) ? `arguments:\n${argsText(toolCall)}` : undefined,
  ]);
}

function resultText(toolCall: ToolCallView): string | undefined {
  if (toolCall.output) {
    return toolCall.output;
  }
  return toolCall.summary;
}

function argsText(toolCall: ToolCallView): string {
  const raw = toolCall.arguments ?? toolCall.argsStream;
  if (!raw.trim()) {
    return "";
  }
  const parsed = parseJson(raw);
  return parsed === undefined ? raw : jsonPreview(parsed);
}

function parsedArgs(
  toolCall: ToolCallView,
): Record<string, unknown> | undefined {
  const parsed = parseJson(toolCall.arguments ?? toolCall.argsStream);
  return isRecord(parsed) ? parsed : undefined;
}

function stringField(
  value: Record<string, unknown> | undefined,
  key: string,
): string | undefined {
  const field = value?.[key];
  return typeof field === "string" ? field : undefined;
}

function compactLines(lines: Array<string | undefined | null | false>): string {
  return lines.filter((line): line is string => Boolean(line)).join("\n");
}

function cappedSection(
  value: string | undefined,
  cap: number,
): string | undefined {
  if (!value) {
    return undefined;
  }
  const lines = value.split(/\r?\n/);
  const shown = lines.slice(0, cap);
  if (lines.length > cap) {
    shown.push(`… +${lines.length - cap} more lines`);
  }
  return shown.join("\n");
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
      return toolLine(cursor, {
        id: block.id,
        name: block.name,
        argsStream: block.args,
        arguments: block.state === "done" ? block.args : undefined,
        state: inFlightToolState(block.state),
      });
  }
}

function inFlightToolState(
  state: InFlightToolCallState | undefined,
): ToolCallState {
  switch (state) {
    case "streaming_args":
      return "streaming_args";
    case "done":
      return "running";
    case "pending":
    default:
      return "pending";
  }
}

function plural(count: number): string {
  return count === 1 ? "" : "s";
}

function stateSuffix(state: ToolCallState): string {
  switch (state) {
    case "pending":
      return "pending";
    case "streaming_args":
      return "streaming args";
    case "running":
      return "running";
    case "done":
      return "done";
    case "error":
      return "error";
  }
}

function firstLine(value: string): string {
  return value.split(/\r?\n/, 1)[0] ?? "";
}

function slugify(value: string): string {
  return value.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(
    /^-|-$/g,
    "",
  ) || "event";
}

function parseJson(value: string): unknown {
  try {
    return JSON.parse(value);
  } catch {
    return undefined;
  }
}

function jsonPreview(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2) ?? "null";
  } catch {
    return String(value);
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
