import type {
  Event as ProtocolEvent,
  Alert,
  InFlightBlock,
  InFlightToolCallState,
  Segment,
} from "$lib/generated/protocol";
import { workspaceRoute } from "$lib/workspace/api/http";

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
  eventId?: string | null;
  source: "event";
  streaming?: boolean;
  error?: boolean;
  toolCall?: ToolCallView;
};

export type ConsoleProjection = {
  lines: ConsoleLine[];
  status: string | null;
  usage: string | null;
  lastEventId: string | null;
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

export type ConsoleEventInput = {
  eventId: string;
  event: ProtocolEvent;
  observedAtMs?: number;
};

export function emptyConsoleProjection(): ConsoleProjection {
  return {
    lines: [],
    status: null,
    usage: null,
    lastEventId: null,
  };
}

export function projectConsole(
  events: ConsoleEventInput[] = [],
): ConsoleProjection {
  const projection = events.reduce(applyProtocolEvent, emptyConsoleProjection());
  return projectVisibleConsole(projection);
}

export function createConsoleProjector() {
  let projection = emptyConsoleProjection();
  return {
    reset(): ConsoleProjection {
      projection = emptyConsoleProjection();
      return projectVisibleConsole(projection);
    },
    append(events: ConsoleEventInput[]): ConsoleProjection {
      for (const event of events) {
        projection = applyProtocolEvent(projection, event);
      }
      return projectVisibleConsole(projection);
    },
    snapshot(): ConsoleProjection {
      return projectVisibleConsole(projection);
    },
  };
}

function projectVisibleConsole(projection: ConsoleProjection): ConsoleProjection {
  return {
    ...projection,
    lines: aggregateReadToolLines(projection.lines),
  };
}

export function applyProtocolEvent(
  projection: ConsoleProjection,
  envelope: { eventId: string; event: ProtocolEvent },
): ConsoleProjection {
  const next: ConsoleProjection = {
    lines: [...projection.lines],
    status: projection.status,
    usage: projection.usage,
    lastEventId: envelope.eventId,
  };
  const event = envelope.event;

  switch (event.event) {
    case "user_message":
      next.lines.push(
        line(
          envelope.eventId,
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
        envelope.eventId,
        "assistant",
        "assistant streaming",
        event.data.text,
      );
      break;
    case "text_done":
      finalizeStreaming(
        next,
        "assistant",
        envelope.eventId,
        "assistant",
        event.data.text,
      );
      break;
    case "thinking_start":
      next.lines.push(
        line(envelope.eventId, "thinking", "Thinking...", "", undefined, true),
      );
      break;
    case "thinking_delta":
      appendStreaming(
        next,
        envelope.eventId,
        "thinking",
        "Thinking...",
        event.data.text,
      );
      break;
    case "thinking_done":
      finalizeStreaming(
        next,
        "thinking",
        envelope.eventId,
        "Thought",
        event.data.text,
      );
      break;
    case "tool_call_start":
      upsertToolCall(next, envelope.eventId, event.data.id, {
        name: event.data.name,
        state: "pending",
      });
      break;
    case "tool_call_args_delta":
      appendToolArgs(next, envelope.eventId, event.data.id, event.data.json);
      break;
    case "tool_call_done":
      upsertToolCall(next, envelope.eventId, event.data.id, {
        name: event.data.name,
        arguments: event.data.arguments,
        argsStream: event.data.arguments,
        state: "running",
      });
      break;
    case "tool_result":
      attachToolResult(next, envelope.eventId, event.data.id, {
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
          envelope.eventId,
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
      next.lines = snapshotLinesFromEntries(envelope.eventId, event.data.entries);
      for (const block of event.data.in_flight?.blocks ?? []) {
        next.lines.push(inFlightLine(envelope.eventId, block));
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
      break;
    case "alert":
      appendAlertLine(next, envelope.eventId, event.data);
      break;
    case "memory_worker":
    case "segment_rotated":
    case "completions":
    case "rewind_targets":
    case "rewind_applied":
    case "workers_listed":
    case "worker_restored":
    case "peer_registered":
      // These are protocol/status/control events. TUI Console does not append
      // them to the conversation surface; browser Console should not either.
      break;
    case "compact_start":
      upsertStatusLine(next, "compact", envelope.eventId, "Compacting…", true);
      break;
    case "compact_done":
      upsertStatusLine(next, "compact", envelope.eventId, "Compacted.", false);
      break;
    case "compact_failed":
      upsertStatusLine(
        next,
        "compact",
        envelope.eventId,
        `Compact failed: ${event.data.error}`,
        false,
        true,
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
        case "unknown":
          return "[unknown segment]";
      }
    })
    .join("\n");
}

function line(
  eventId: string,
  kind: ConsoleLineKind,
  title: string,
  body: string,
  detail?: string,
  streaming = false,
  error = false,
): ConsoleLine {
  return {
    id: `event-${eventId}-${kind}-${slugify(title)}-${body.length}`,
    kind,
    title,
    body,
    detail,
    eventId,
    source: "event",
    streaming,
    error,
  };
}

function upsertStatusLine(
  projection: ConsoleProjection,
  id: string,
  eventId: string,
  body: string,
  streaming: boolean,
  error = false,
): void {
  const lineId = `status-${id}`;
  const index = projection.lines.findIndex((item) => item.id === lineId);
  const item: ConsoleLine = {
    id: lineId,
    kind: error ? "error" : "status",
    title: error ? "Status error" : "Status",
    body,
    eventId,
    source: "event",
    streaming,
    error,
  };
  if (index >= 0) {
    projection.lines[index] = item;
  } else {
    projection.lines.push(item);
  }
}

function appendAlertLine(
  projection: ConsoleProjection,
  eventId: string,
  alert: Alert,
): void {
  const isError = alert.level === "error";
  projection.lines.push(
    line(
      eventId,
      isError ? "error" : "status",
      `Alert · ${alert.source}`,
      alert.message,
      undefined,
      false,
      isError,
    ),
  );
}

function findLastLineIndex(
  lines: ConsoleLine[],
  predicate: (line: ConsoleLine) => boolean,
): number {
  for (let index = lines.length - 1; index >= 0; index -= 1) {
    if (predicate(lines[index])) {
      return index;
    }
  }
  return -1;
}

function appendStreaming(
  projection: ConsoleProjection,
  eventId: string,
  kind: "assistant" | "thinking",
  title: string,
  delta: string,
): void {
  const index = findLastLineIndex(
    projection.lines,
    (item) => item.kind === kind && item.streaming === true,
  );
  if (index >= 0) {
    const existing = projection.lines[index];
    projection.lines[index] = {
      ...existing,
      body: `${existing.body}${delta}`,
      eventId,
    };
    return;
  }
  projection.lines.push(line(eventId, kind, title, delta, undefined, true));
}

function finalizeStreaming(
  projection: ConsoleProjection,
  kind: "assistant" | "thinking",
  eventId: string,
  title: string,
  body: string,
): void {
  const index = findLastLineIndex(
    projection.lines,
    (item) => item.kind === kind && item.streaming === true,
  );
  if (index >= 0) {
    const existing = projection.lines[index];
    projection.lines[index] = {
      ...existing,
      body: body || existing.body,
      streaming: false,
      title,
      eventId,
    };
    return;
  }
  projection.lines.push(line(eventId, kind, title, body));
}

function upsertToolCall(
  projection: ConsoleProjection,
  eventId: string,
  id: string,
  update: Partial<Omit<ToolCallView, "id">>,
): ConsoleLine {
  const index = findToolCallLineIndex(projection, id);
  if (index < 0) {
    const created = toolLine(eventId, {
      id,
      name: update.name ?? "Tool",
      argsStream: update.argsStream ?? "",
      arguments: update.arguments,
      state: update.state ?? "pending",
      summary: update.summary,
      output: update.output,
      isError: update.isError,
    });
    projection.lines.push(created);
    return created;
  }

  const existing = projection.lines[index];
  const updated = refreshedToolLine({
    ...existing,
    eventId,
    toolCall: {
      ...existing.toolCall!,
      ...update,
      id,
      name: update.name ?? existing.toolCall!.name,
    },
  });
  projection.lines[index] = updated;
  return updated;
}

function appendToolArgs(
  projection: ConsoleProjection,
  eventId: string,
  id: string,
  delta: string,
): void {
  const index = findToolCallLineIndex(projection, id);
  if (index < 0) {
    projection.lines.push(toolLine(eventId, {
      id,
      name: "Tool",
      argsStream: delta,
      state: "streaming_args",
    }));
    return;
  }
  const existing = projection.lines[index];
  const toolCall = existing.toolCall!;
  projection.lines[index] = refreshedToolLine({
    ...existing,
    eventId,
    toolCall: {
      ...toolCall,
      argsStream: `${toolCall.argsStream}${delta}`,
      state: "streaming_args",
    },
  });
}

function attachToolResult(
  projection: ConsoleProjection,
  eventId: string,
  id: string,
  result: Pick<ToolCallView, "summary" | "output" | "isError">,
): void {
  const index = findToolCallLineIndex(projection, id);
  if (index < 0) {
    const fallback = refreshedToolLine({
      ...toolLine(eventId, {
        id,
        name: "Tool",
        argsStream: "",
        state: result.isError ? "error" : "done",
        ...result,
      }),
      title: result.isError ? "Call · Tool result error" : "Call · Tool result",
    });
    projection.lines.push(fallback);
    return;
  }
  const existing = projection.lines[index];
  projection.lines[index] = refreshedToolLine({
    ...existing,
    eventId,
    toolCall: {
      ...existing.toolCall!,
      ...result,
      state: result.isError ? "error" : "done",
    },
  });
}

function findToolCallLineIndex(
  projection: ConsoleProjection,
  id: string,
): number {
  for (let index = projection.lines.length - 1; index >= 0; index -= 1) {
    if (projection.lines[index].toolCall?.id === id) {
      return index;
    }
  }
  return -1;
}

function toolLine(eventId: string, toolCall: ToolCallView): ConsoleLine {
  return refreshedToolLine({
    id: `tool-call-${toolCall.id}`,
    kind: "tool",
    title: `Call · ${toolCall.name}`,
    body: "",
    detail: undefined,
    eventId,
    source: "event",
    streaming: true,
    error: false,
    toolCall,
  });
}

function refreshedToolLine(item: ConsoleLine): ConsoleLine {
  const toolCall = item.toolCall;
  if (!toolCall) {
    return item;
  }
  return {
    ...item,
    title: item.title.startsWith("Call · Tool result")
      ? item.title
      : `Call · ${toolCall.name}`,
    body: renderToolCall(toolCall),
    detail: toolCallDetail(toolCall),
    diff: toolCall.name === "Edit" ? editDiff(toolCall) : undefined,
    streaming: !["done", "error"].includes(toolCall.state),
    error: toolCall.state === "error",
  };
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
      return renderSearchTool(toolCall);
    case "Grep":
      return renderGrepTool(toolCall);
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
    eventId: group.at(-1)?.eventId,
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
    `${toolCall.name} — ${toolHeaderSuffix(toolCall, summary)}`,
    resultText(toolCall),
  ]);
}

function renderGrepTool(toolCall: ToolCallView): string {
  const summary = toolCall.summary?.trim();
  return compactLines([
    `Grep — ${toolHeaderSuffix(toolCall, summary)}`,
    grepQueryText(toolCall),
    cappedResultSection(resultText(toolCall), 5),
  ]);
}

function toolHeaderSuffix(
  toolCall: ToolCallView,
  summary?: string,
): string {
  if (toolCall.state === "error") {
    return "Failed";
  }
  return summary ? firstLine(summary) : stateSuffix(toolCall.state);
}

function grepQueryText(toolCall: ToolCallView): string | undefined {
  const args = parsedArgs(toolCall);
  const pattern = stringField(args, "pattern");
  if (pattern) {
    return `query: ${pattern}`;
  }
  const renderedArgs = argsText(toolCall);
  return renderedArgs ? `query:\n${renderedArgs}` : undefined;
}

function renderBashTool(toolCall: ToolCallView): string {
  const args = parsedArgs(toolCall);
  const command = stringField(args, "command");
  return compactLines([
    `Bash — ${stateSuffix(toolCall.state)}`,
    command ? `$ ${command}` : argsText(toolCall),
    cappedDisplaySection(resultText(toolCall), 10),
  ]);
}

function renderDefaultTool(toolCall: ToolCallView): string {
  return compactLines([
    `${toolCall.name} — ${stateSuffix(toolCall.state)}`,
    cappedDisplaySection(argsText(toolCall), 3),
    cappedDisplaySection(resultText(toolCall), 3),
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

function arrayField(value: Record<string, unknown>, key: string): unknown[] {
  const field = value[key];
  return Array.isArray(field) ? field : [];
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

function cappedDisplaySection(
  value: string | undefined,
  maxLines: number,
): string | undefined {
  if (!value) {
    return undefined;
  }
  const lines = value.split(/\r?\n/);
  if (lines.length <= maxLines) {
    return value;
  }
  if (maxLines <= 0) {
    return undefined;
  }
  const shown = lines.slice(0, maxLines);
  shown[maxLines - 1] = `… +${lines.length - maxLines + 1} more lines`;
  return shown.join("\n");
}

function cappedResultSection(
  value: string | undefined,
  maxResults: number,
): string | undefined {
  if (!value || maxResults <= 0) {
    return undefined;
  }
  const lines = value.split(/\r?\n/).filter((line) => line.length > 0);
  if (lines.length <= maxResults) {
    return lines.join("\n");
  }
  return [
    ...lines.slice(0, maxResults),
    `… +${lines.length - maxResults} more results`,
  ].join("\n");
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

function snapshotLinesFromEntries(eventId: string, entries: unknown[]): ConsoleLine[] {
  const projection: ConsoleProjection = {
    lines: [],
    status: null,
    usage: null,
    lastEventId: eventId,
  };
  entries.forEach((entry, index) => applyLogEntry(projection, `${eventId}-snapshot-${index}`, entry));
  return projection.lines;
}

function applyLogEntry(
  projection: ConsoleProjection,
  eventId: string,
  entry: unknown,
): void {
  if (!isRecord(entry)) return;
  switch (stringField(entry, "kind")) {
    case "segment_start":
      arrayField(entry, "history").forEach((item, index) =>
        applyLoggedItem(projection, `${eventId}-history-${index}`, item)
      );
      break;
    case "user_input":
      projection.lines.push(
        line(
          eventId,
          "user",
          "User",
          segmentsToText(arrayField(entry, "segments") as Segment[]),
        ),
      );
      break;
    case "assistant_item":
    case "tool_result":
      applyLoggedItem(projection, eventId, entry["item"]);
      break;
    case "extension":
      applyExtensionEntry(projection, eventId, entry);
      break;
    default:
      break;
  }
}

function applyExtensionEntry(
  projection: ConsoleProjection,
  eventId: string,
  entry: Record<string, unknown>,
) {
  if (entry["domain"] !== "yoi.compaction") {
    return;
  }
  const payload = entry["payload"];
  if (!isRecord(payload) || payload["kind"] !== "compaction_block") {
    return;
  }
  const blockId = stringField(payload, "block_id") || "compact";
  const state = stringField(payload, "state") || "running";
  const message = stringField(payload, "message") || compactMessageForState(state, payload);
  upsertStatusLine(
    projection,
    blockId,
    eventId,
    message,
    state === "running",
    state === "failed",
  );
}

function compactMessageForState(
  state: string,
  payload: Record<string, unknown>,
): string {
  switch (state) {
    case "done":
      return "Compacted.";
    case "failed": {
      const error = stringField(payload, "error");
      return error ? `Compact failed: ${error}` : "Compact failed.";
    }
    default:
      return "Compacting…";
  }
}

function applyLoggedItem(
  projection: ConsoleProjection,
  eventId: string,
  item: unknown,
): void {
  if (!isRecord(item)) return;
  switch (stringField(item, "kind")) {
    case "message": {
      const body = loggedContentText(arrayField(item, "content"));
      switch (stringField(item, "role")) {
        case "user":
          projection.lines.push(line(eventId, "user", "User", body));
          break;
        case "assistant":
          projection.lines.push(line(eventId, "assistant", "assistant", body));
          break;
        default:
          break;
      }
      break;
    }
    case "reasoning": {
      const text = stringField(item, "text") ?? arrayField(item, "summary").filter((value) => typeof value === "string").join("\n");
      if (text) {
        projection.lines.push(line(eventId, "thinking", "Thought", text));
      }
      break;
    }
    case "tool_call":
      upsertToolCall(projection, eventId, stringField(item, "call_id") ?? eventId, {
        name: stringField(item, "name") ?? "Tool",
        arguments: stringField(item, "arguments") ?? "",
        argsStream: stringField(item, "arguments") ?? "",
        state: "running",
      });
      break;
    case "tool_result":
      attachToolResult(projection, eventId, stringField(item, "call_id") ?? eventId, {
        summary: stringField(item, "summary") ?? "",
        output: stringField(item, "content"),
        isError: item["is_error"] === true,
      });
      break;
    default:
      break;
  }
}

function loggedContentText(parts: unknown[]): string {
  return parts
    .map((part) => {
      if (!isRecord(part)) return "";
      switch (stringField(part, "kind")) {
        case "text":
          return stringField(part, "text") ?? "";
        case "refusal":
          return stringField(part, "refusal") ?? "";
        default:
          return "";
      }
    })
    .filter(Boolean)
    .join("\n");
}

function inFlightLine(eventId: string, block: InFlightBlock): ConsoleLine {
  switch (block.kind) {
    case "text":
      return line(
        eventId,
        "in_flight",
        "in-flight assistant text",
        block.text,
        undefined,
        !block.finished,
      );
    case "thinking":
      return line(
        eventId,
        "in_flight",
        "in-flight thinking",
        block.text,
        undefined,
        !block.finished,
      );
    case "tool_call":
      return toolLine(eventId, {
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
