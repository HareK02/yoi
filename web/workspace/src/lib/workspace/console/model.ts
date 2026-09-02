import type {
  Alert,
  CommandEvent,
  CommandSnapshot,
  CommandStreamSlice,
  CompactionLifecycle,
  Event as ProtocolEvent,
  InFlightBlock,
  InFlightToolCallState,
  InternalWorkerRef,
  InternalWorkerSnapshot,
  Segment,
} from "$lib/generated/protocol";
import { stringify as stringifyYaml } from "yaml";
import { workspaceRoute } from "$lib/workspace/api/http";
import {
  applyRunActivityEvent,
  emptyRunActivityStats,
  formatRunElapsedCompact,
  formatRunTokens,
  type RunActivityStats,
} from "./run-status.ts";
import {
  applyTaskSnapshotText,
  applyTaskToolCall,
  type ConsoleTask,
} from "./tasks.ts";

export type ConsoleLineKind =
  | "user"
  | "assistant"
  | "thinking"
  | "tool"
  | "activity"
  | "task_reminder"
  | "run_stats"
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
  cwd?: string | null;
  command?: CommandSnapshot;
};

export type ConsoleDiffLine = {
  kind: "context" | "add" | "remove";
  oldNumber?: number;
  newNumber?: number;
  content: string;
};

export type ConsoleViewMode = "overview" | "normal";

export type ConsoleCompaction = {
  id: string;
  revision: number;
  state: "running" | "done" | "failed" | "interrupted";
  startedAtMs: number;
  endedAtMs?: number;
  summary?: string;
  candidate?: string;
  error?: string;
  internalWorkerSessionId?: string;
  activity: string[];
};

export type ConsoleLine = {
  id: string;
  kind: ConsoleLineKind;
  title: string;
  body: string;
  expandedBody?: string;
  toolCallLabel?: string;
  toolStatus?: string;
  detail?: string;
  compaction?: ConsoleCompaction;
  diff?: ConsoleDiffLine[];
  eventId?: string | null;
  source: "event";
  streaming?: boolean;
  error?: boolean;
  toolCall?: ToolCallView;
  /** Number of calls represented by a lower-level aggregate line. */
  toolCallCount?: number;
  /** Typed `SystemItem.kind` used by presentation-only projections. */
  systemItemKind?: string;
};

export type InternalWorkerProjection = {
  worker: InternalWorkerRef;
  revision: number;
  console: ConsoleProjection;
};

export type ConsoleViewScroll = {
  top: number;
  autoFollow: boolean;
};

export function resolveConsoleViewScrollTop(
  state: ConsoleViewScroll | undefined,
  scrollHeight: number,
  clientHeight: number,
): number {
  if (!state || state.autoFollow) return scrollHeight;
  return Math.min(state.top, Math.max(0, scrollHeight - clientHeight));
}

export type ConsoleWorkerView = {
  sessionId: string | null;
  label: string;
  console: ConsoleProjection;
};

export function consoleWorkerViews(
  projection: ConsoleProjection,
): ConsoleWorkerView[] {
  const children = projection.internalWorkers.filter((worker) =>
    worker.worker.kind === "sub_worker"
  );
  const labels = children.map((worker) => worker.worker.name || "subworker");
  const labelCounts = new Map<string, number>();
  for (const label of labels) {
    labelCounts.set(label, (labelCounts.get(label) ?? 0) + 1);
  }
  return [
    { sessionId: null, label: "main", console: projection },
    ...children.map((worker, index) => {
      const label = labels[index] ?? "subworker";
      return {
        sessionId: worker.worker.session_id,
        label: labelCounts.get(label) === 1
          ? label
          : `${label} · ${worker.worker.session_id.slice(-6)}`,
        console: worker.console,
      };
    }),
  ];
}

export function resolveConsoleWorkerView(
  projection: ConsoleProjection,
  selectedSessionId: string | null,
): ConsoleWorkerView {
  const views = consoleWorkerViews(projection);
  return views.find((view) => view.sessionId === selectedSessionId) ?? views[0];
}

export type ConsoleProjection = {
  lines: ConsoleLine[];
  tasks: ConsoleTask[];
  taskNextId: number;
  status: string | null;
  usage: string | null;
  runActivity: RunActivityStats;
  cwd: string | null;
  lastEventId: string | null;
  internalWorkers: InternalWorkerProjection[];
  /** Terminal child-session fences, reset only by an authoritative snapshot. */
  removedInternalWorkers: Record<string, number>;
};

export type ConsoleTimelineLineSelection = {
  item: ConsoleLine;
  index: number;
};

export function selectConsoleTimelineLines(
  items: ConsoleLine[],
): ConsoleTimelineLineSelection[] {
  const selected: ConsoleTimelineLineSelection[] = [];
  let lastAssistant: ConsoleTimelineLineSelection | null = null;

  const flushAssistant = () => {
    if (lastAssistant) {
      selected.push(lastAssistant);
      lastAssistant = null;
    }
  };

  items.forEach((item, index) => {
    if (item.kind === "user") {
      flushAssistant();
      selected.push({ item, index });
      return;
    }
    if (item.kind === "assistant") {
      lastAssistant = { item, index };
    }
  });

  flushAssistant();
  return selected;
}

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

export function isConsoleProjectionEvent(event: ProtocolEvent): boolean {
  return event.event !== "completions";
}

export function emptyConsoleProjection(): ConsoleProjection {
  return {
    lines: [],
    tasks: [],
    taskNextId: 1,
    status: null,
    usage: null,
    runActivity: emptyRunActivityStats(),
    cwd: null,
    lastEventId: null,
    internalWorkers: [],
    removedInternalWorkers: {},
  };
}

export function projectConsole(
  events: ConsoleEventInput[] = [],
): ConsoleProjection {
  const projection = events.reduce(
    applyProtocolEvent,
    emptyConsoleProjection(),
  );
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

function projectVisibleConsole(
  projection: ConsoleProjection,
): ConsoleProjection {
  return {
    ...projection,
    lines: aggregateReadToolLines(projection.lines),
    internalWorkers: projection.internalWorkers.map((worker) => ({
      ...worker,
      console: projectVisibleConsole(worker.console),
    })),
  };
}

function isOverviewThinkingLine(line: ConsoleLine): boolean {
  return line.kind === "thinking" ||
    (line.kind === "in_flight" && line.title === "in-flight thinking");
}

function representedToolCallCount(line: ConsoleLine): number {
  return Math.max(1, line.toolCallCount ?? 1);
}

function overviewToolActivityLine(group: ConsoleLine[]): ConsoleLine {
  const first = group[0]!;
  const last = group[group.length - 1]!;
  let readCount = 0;
  let searchCount = 0;
  let commandCount = 0;
  let editCount = 0;
  let writeCount = 0;
  let additions = 0;
  let deletions = 0;
  let failedCount = 0;
  let activeCount = 0;
  let readActive = false;
  let searchActive = false;
  let commandActive = false;
  let editActive = false;
  let writeActive = false;
  const otherCounts = new Map<string, number>();

  for (const line of group) {
    const count = representedToolCallCount(line);
    const name = line.toolCall?.name ?? "Tool";
    const state = line.toolCall?.state;
    if (state === "error" || line.error || line.toolCall?.isError) {
      failedCount += count;
    }
    const callActive = state === "pending" || state === "streaming_args" ||
      state === "running";
    if (callActive) activeCount += count;

    switch (name) {
      case "Read":
        readCount += count;
        readActive ||= callActive;
        break;
      case "Glob":
      case "Grep":
      case "WebSearch":
      case "SearchSessionEntries":
        searchCount += count;
        searchActive ||= callActive;
        break;
      case "Bash":
        commandCount += count;
        commandActive ||= callActive;
        break;
      case "Edit":
        editCount += count;
        editActive ||= callActive;
        if (state === "done") {
          additions += line.diff?.filter((diff) =>
            diff.kind === "add"
          ).length ?? 0;
          deletions += line.diff?.filter((diff) =>
            diff.kind === "remove"
          ).length ?? 0;
        }
        break;
      case "Write":
        writeCount += count;
        writeActive ||= callActive;
        break;
      default:
        otherCounts.set(name, (otherCounts.get(name) ?? 0) + count);
        break;
    }
  }

  const active = activeCount > 0;
  const primary: string[] = [];
  if (readCount > 0) {
    primary.push(
      readActive
        ? `reading ${readCount} file${readCount === 1 ? "" : "s"}`
        : `${readCount} file${readCount === 1 ? "" : "s"} read`,
    );
  }
  if (searchCount > 0) {
    primary.push(
      searchActive
        ? `searching ${searchCount} time${searchCount === 1 ? "" : "s"}`
        : `searched ${searchCount} time${searchCount === 1 ? "" : "s"}`,
    );
  }
  if (commandCount > 0) {
    primary.push(
      commandActive
        ? `running ${commandCount} command${commandCount === 1 ? "" : "s"}`
        : `ran ${commandCount} command${commandCount === 1 ? "" : "s"}`,
    );
  }
  for (
    const [name, count] of [...otherCounts].sort(([left], [right]) =>
      left.localeCompare(right)
    )
  ) {
    primary.push(count === 1 ? name : `${count} ${name}`);
  }

  const changes: string[] = [];
  if (editCount > 0) {
    if (editActive) {
      changes.push(`editing ${editCount} file${editCount === 1 ? "" : "s"}`);
    } else if (additions > 0 || deletions > 0) {
      changes.push(`edited +${additions}/-${deletions}`);
    } else {
      changes.push(`edited ${editCount} file${editCount === 1 ? "" : "s"}`);
    }
  }
  if (writeCount > 0) {
    changes.push(
      writeActive
        ? `writing ${writeCount} file${writeCount === 1 ? "" : "s"}`
        : `wrote ${writeCount} file${writeCount === 1 ? "" : "s"}`,
    );
  }
  if (failedCount > 0) changes.push(`${failedCount} failed`);

  return {
    id: `activity-${first.id}-${last.id}`,
    kind: "activity",
    title: "Activity",
    body: [primary.join("・"), ...changes].filter(Boolean).join("\n"),
    source: "event",
    streaming: active,
    error: failedCount > 0,
  };
}

/**
 * Builds the overview-only Console presentation. Protocol projection retains
 * full tool and thinking state for reconciliation, but the visible history
 * hides thinking and folds each uninterrupted tool run into one activity.
 */
export function projectOverviewLines(lines: ConsoleLine[]): ConsoleLine[] {
  const overview: ConsoleLine[] = [];
  let toolGroup: ConsoleLine[] = [];

  const flushTools = () => {
    if (toolGroup.length === 0) return;
    overview.push(overviewToolActivityLine(toolGroup));
    toolGroup = [];
  };

  for (const line of lines) {
    if (line.systemItemKind === "task_reminder") continue;
    if (isOverviewThinkingLine(line)) continue;
    if (line.kind === "tool" && line.toolCall) {
      toolGroup.push(line);
      continue;
    }
    flushTools();
    overview.push(line);
  }
  flushTools();
  return overview;
}

export function projectNormalLines(lines: ConsoleLine[]): ConsoleLine[] {
  return lines.map((line) => {
    if (line.systemItemKind !== "task_reminder") return line;
    const first = line.body
      .split("\n")
      .map((part) => part.trim())
      .find(Boolean);
    return {
      ...line,
      kind: "task_reminder",
      title: "Task reminder",
      body: first ? `task reminder: ${first}` : "task reminder",
      detail: undefined,
    };
  });
}

export function projectConsoleLines(
  lines: ConsoleLine[],
  mode: ConsoleViewMode,
): ConsoleLine[] {
  return mode === "overview"
    ? projectOverviewLines(lines)
    : projectNormalLines(lines);
}

function appendSnapshotInFlightLines(
  projection: ConsoleProjection,
  blocks: InFlightBlock[],
  eventId: string,
  cwd: string | null,
): void {
  const lineIds = new Set(projection.lines.map((line) => line.id));
  blocks.forEach((block, index) => {
    const pending = inFlightLine(`${eventId}:${index}`, block, cwd);
    if (lineIds.has(pending.id)) return;
    projection.lines.push(pending);
    lineIds.add(pending.id);
  });
}

const COMMAND_STREAM_DISPLAY_BYTES = 32 * 1024;

function appendSnapshotCommands(
  projection: ConsoleProjection,
  commands: CommandSnapshot[],
  eventId: string,
): void {
  commands.forEach((command) => upsertCommandSnapshot(projection, eventId, command));
}

function upsertCommandSnapshot(
  projection: ConsoleProjection,
  eventId: string,
  command: CommandSnapshot,
): void {
  const toolCallId = command.tool_call_id ?? `command:${command.command_id}`;
  const existingIndex = findToolCallLineIndex(projection, toolCallId);
  const existing = existingIndex >= 0
    ? projection.lines[existingIndex].toolCall
    : undefined;
  upsertToolCall(projection, eventId, toolCallId, {
    name: existing?.name ?? "Bash",
    state: existing?.state ?? "running",
    command,
  });
}

function applyCommandEvent(
  projection: ConsoleProjection,
  eventId: string,
  event: CommandEvent,
): void {
  if (event.kind === "started") {
    upsertCommandSnapshot(projection, eventId, {
      command_id: event.command_id,
      tool_call_id: event.tool_call_id,
      status: "running",
      started_at_ms: event.observed_at_ms,
      observed_at_ms: event.observed_at_ms,
      last_output_at_ms: null,
      stdout: emptyCommandStream(),
      stderr: emptyCommandStream(),
      exit_code: null,
    });
    return;
  }

  const index = projection.lines.findIndex((line) =>
    line.toolCall?.command?.command_id === event.command_id
  );
  if (index < 0) {
    if (event.kind === "output") {
      const stream = commandStreamFromEvent(event);
      upsertCommandSnapshot(projection, eventId, {
        command_id: event.command_id,
        tool_call_id: null,
        status: "running",
        started_at_ms: event.observed_at_ms,
        observed_at_ms: event.observed_at_ms,
        last_output_at_ms: event.observed_at_ms,
        stdout: event.stream === "stdout" ? stream : emptyCommandStream(),
        stderr: event.stream === "stderr" ? stream : emptyCommandStream(),
        exit_code: null,
      });
    }
    return;
  }

  const existing = projection.lines[index].toolCall!.command!;
  if (event.kind === "terminal") {
    upsertCommandSnapshot(projection, eventId, {
      ...existing,
      status: event.status,
      exit_code: event.exit_code,
      observed_at_ms: event.observed_at_ms,
    });
    return;
  }
  const updatedStream = appendCommandStream(
    event.stream === "stdout" ? existing.stdout : existing.stderr,
    event.start_offset,
    event.end_offset,
    event.content,
  );
  upsertCommandSnapshot(projection, eventId, {
    ...existing,
    observed_at_ms: event.observed_at_ms,
    last_output_at_ms: event.observed_at_ms,
    stdout: event.stream === "stdout" ? updatedStream : existing.stdout,
    stderr: event.stream === "stderr" ? updatedStream : existing.stderr,
  });
}

function emptyCommandStream(): CommandStreamSlice {
  return { start_offset: 0, end_offset: 0, content: "", truncated: false };
}

function commandStreamFromEvent(
  event: Extract<CommandEvent, { kind: "output" }>,
): CommandStreamSlice {
  return appendCommandStream(
    emptyCommandStream(),
    event.start_offset,
    event.end_offset,
    event.content,
  );
}

function appendCommandStream(
  existing: CommandStreamSlice,
  startOffset: number,
  endOffset: number,
  content: string,
): CommandStreamSlice {
  if (endOffset <= existing.end_offset) return existing;
  const contiguous = startOffset === existing.end_offset;
  const combined = contiguous ? `${existing.content}${content}` : content;
  const tail = combined.length > COMMAND_STREAM_DISPLAY_BYTES
    ? combined.slice(-COMMAND_STREAM_DISPLAY_BYTES)
    : combined;
  return {
    start_offset: endOffset - tail.length,
    end_offset: endOffset,
    content: tail,
    truncated: existing.truncated || !contiguous || tail.length < combined.length ||
      startOffset > 0,
  };
}

function projectInternalWorkerSnapshot(
  snapshot: InternalWorkerSnapshot,
  eventId: string,
  cwd: string | null,
): InternalWorkerProjection {
  const console = snapshotProjectionFromSession(
    `${eventId}:internal:${snapshot.worker.session_id}:snapshot`,
    snapshot.session,
    cwd,
  );
  console.status = snapshot.status;
  appendSnapshotInFlightLines(
    console,
    snapshot.in_flight?.blocks ?? [],
    `${eventId}:internal:${snapshot.worker.session_id}:in-flight`,
    cwd,
  );
  appendSnapshotCommands(
    console,
    snapshot.in_flight?.commands ?? [],
    `${eventId}:internal:${snapshot.worker.session_id}:command`,
  );
  if (snapshot.error) {
    console.lines.push({
      id: `${eventId}:internal:${snapshot.worker.session_id}:error`,
      kind: "error",
      title: "Error",
      body: snapshot.error,
      eventId,
      source: "event",
      error: true,
    });
  }
  console.internalWorkers = (snapshot.internal_workers ?? []).map((child) =>
    projectInternalWorkerSnapshot(child, eventId, cwd)
  );
  return { worker: snapshot.worker, revision: snapshot.revision, console };
}

function compactionCandidate(
  projection: ConsoleProjection,
  sessionId: string | undefined,
): string | undefined {
  if (!sessionId) return undefined;
  const worker = projection.internalWorkers.find(
    (candidate) => candidate.worker.session_id === sessionId,
  );
  const calls = worker?.console.lines
    .map((line) => line.toolCall)
    .filter((call): call is ToolCallView => call?.name === "write_summary") ?? [];
  const latest = calls.at(-1);
  const raw = latest?.arguments ?? latest?.argsStream;
  if (!raw) return undefined;
  try {
    const parsed = JSON.parse(raw) as { text?: unknown };
    return typeof parsed.text === "string" ? parsed.text : undefined;
  } catch {
    return undefined;
  }
}

function compactionActivity(
  projection: ConsoleProjection,
  sessionId: string | undefined
): string[] {
  if (!sessionId) return [];
  const worker = projection.internalWorkers.find(
    (candidate) => candidate.worker.session_id === sessionId
  );
  if (!worker) return [];
  return worker.console.lines
    .filter((line) => line.kind === "tool" || line.kind === "status" || line.kind === "error")
    .slice(-12)
    .map((line) =>
      line.toolCall?.name === "write_summary"
        ? `write_summary — ${line.toolCall.state}`
        : line.body || line.title
    )
    .filter((value, index, values) => value.length > 0 && values.indexOf(value) === index);
}

function applyCompactionLifecycle(
  projection: ConsoleProjection,
  lifecycle: CompactionLifecycle
): ConsoleProjection {
  const lineId = `compaction-${lifecycle.compaction_id}`;
  const existing = projection.lines.find((line) => line.id === lineId)?.compaction;
  if (existing && existing.revision >= lifecycle.revision) return projection;
  const internalWorkerSessionId = lifecycle.internal_worker?.session_id;
  const compaction: ConsoleCompaction = {
    id: lifecycle.compaction_id,
    revision: lifecycle.revision,
    state: lifecycle.state,
    startedAtMs: lifecycle.started_at_ms,
    endedAtMs: lifecycle.ended_at_ms ?? undefined,
    summary: lifecycle.summary ?? undefined,
    candidate: compactionCandidate(projection, internalWorkerSessionId),
    error: lifecycle.error ?? undefined,
    internalWorkerSessionId,
    activity: compactionActivity(projection, internalWorkerSessionId)
  };
  const line: ConsoleLine = {
    id: lineId,
    source: "event",
    kind: compaction.state === "failed" ? "error" : "status",
    title: "Compaction",
    body: compaction.summary ?? compaction.error ?? "",
    streaming: compaction.state === "running",
    error: compaction.state === "failed",
    compaction
  };
  const index = projection.lines.findIndex((candidate) => candidate.id === lineId);
  const lines = [...projection.lines];
  if (index >= 0) lines[index] = line;
  else lines.push(line);
  return { ...projection, lines };
}

function refreshCompactionActivity(
  projection: ConsoleProjection,
  sessionId: string
): ConsoleProjection {
  let changed = false;
  const lines = projection.lines.map((line) => {
    if (line.compaction?.internalWorkerSessionId !== sessionId) return line;
    changed = true;
    return {
      ...line,
      compaction: {
        ...line.compaction,
        candidate: compactionCandidate(projection, sessionId),
        activity: compactionActivity(projection, sessionId)
      }
    };
  });
  return changed ? { ...projection, lines } : projection;
}

export function applyProtocolEvent(
  projection: ConsoleProjection,
  envelope: ConsoleEventInput,
): ConsoleProjection {
  const event = envelope.event;
  const next: ConsoleProjection = {
    lines: [...projection.lines],
    tasks: [...projection.tasks],
    taskNextId: projection.taskNextId,
    status: projection.status,
    usage: projection.usage,
    runActivity: applyRunActivityEvent(
      projection.runActivity,
      event,
      envelope.observedAtMs ?? 0,
    ),
    cwd: projection.cwd,
    lastEventId: envelope.eventId,
    internalWorkers: [...projection.internalWorkers],
    removedInternalWorkers: { ...projection.removedInternalWorkers },
  };

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
      next.lines.push(systemItemLine(envelope.eventId, event.data.item));
      applyTaskSystemItem(next, event.data.item);
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
      applyTaskTool(next, event.data.name, event.data.arguments);
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
    case "snapshot": {
      next.status = event.data.status;
      next.cwd = event.data.greeting.cwd;
      const snapshot = snapshotProjectionFromSession(
        envelope.eventId,
        event.data.session,
        next.cwd,
      );
      next.lines = snapshot.lines;
      next.tasks = snapshot.tasks;
      next.taskNextId = snapshot.taskNextId;
      appendSnapshotInFlightLines(
        next,
        event.data.in_flight?.blocks ?? [],
        `${envelope.eventId}:snapshot-in-flight`,
        next.cwd,
      );
      appendSnapshotCommands(
        next,
        event.data.in_flight?.commands ?? [],
        `${envelope.eventId}:snapshot-command`,
      );
      next.internalWorkers = (event.data.internal_workers ?? []).map((worker) =>
        projectInternalWorkerSnapshot(worker, envelope.eventId, next.cwd)
      );
      next.removedInternalWorkers = {};
      for (const line of next.lines) {
        const compaction = line.compaction;
        if (!compaction) continue;
        const sessionId = compaction.internalWorkerSessionId;
        if (sessionId) {
          line.compaction = {
            ...compaction,
            candidate: compactionCandidate(next, sessionId),
            activity: compactionActivity(next, sessionId),
          };
        }
        if (
          compaction.state === "running" &&
          (!sessionId || !next.internalWorkers.some((worker) =>
            worker.worker.session_id === sessionId
          ))
        ) {
          line.streaming = false;
          line.compaction = {
            ...line.compaction!,
            state: "interrupted",
            endedAtMs: envelope.observedAtMs ?? Date.now(),
          };
        }
      }
      break;
    }
    case "internal_worker": {
      if (
        Object.hasOwn(
          next.removedInternalWorkers,
          event.data.worker.session_id,
        )
      ) break;
      const existingIndex = next.internalWorkers.findIndex((worker) =>
        worker.worker.session_id === event.data.worker.session_id
      );
      const existing = existingIndex >= 0
        ? next.internalWorkers[existingIndex]
        : {
          worker: event.data.worker,
          revision: 0,
          console: emptyConsoleProjection(),
        };
      if (event.data.revision <= existing.revision) break;
      const updated: InternalWorkerProjection = {
        worker: event.data.worker,
        revision: event.data.revision,
        console: applyProtocolEvent(existing.console, {
          eventId:
            `${envelope.eventId}:internal:${event.data.worker.session_id}:${event.data.revision}`,
          event: event.data.event,
          observedAtMs: envelope.observedAtMs,
        }),
      };
      if (existingIndex >= 0) next.internalWorkers[existingIndex] = updated;
      else next.internalWorkers.push(updated);
      return refreshCompactionActivity(next, event.data.worker.session_id);
    }
    case "internal_worker_removed": {
      const existingIndex = next.internalWorkers.findIndex((worker) =>
        worker.worker.session_id === event.data.worker.session_id
      );
      const existingRevision = existingIndex >= 0
        ? next.internalWorkers[existingIndex].revision
        : 0;
      if (event.data.revision <= existingRevision) break;
      next.removedInternalWorkers[event.data.worker.session_id] =
        event.data.revision;
      if (existingIndex >= 0) next.internalWorkers.splice(existingIndex, 1);
      break;
    }
    case "status":
      next.status = event.data.status;
      break;
    case "command":
      applyCommandEvent(next, envelope.eventId, event.data.event);
      break;
    case "segment_rotated": {
      const retainedErrors = next.lines.filter((line) => line.kind === "error");
      const segment = snapshotProjectionFromSession(
        envelope.eventId,
        event.data.session,
        next.cwd,
      );
      next.lines = [...segment.lines, ...retainedErrors];
      next.tasks = segment.tasks;
      next.taskNextId = segment.taskNextId;
      break;
    }
    case "invoke_start":
    case "turn_start":
    case "turn_end":
    case "llm_call_start":
    case "llm_call_end":
    case "llm_retry":
    case "llm_continuation":
      break;
    case "run_end":
      next.lines.push(
        runStatsLine(
          envelope.eventId,
          next.runActivity,
          envelope.observedAtMs ?? next.runActivity.startedAtMs ?? 0,
        ),
      );
      break;
    case "alert":
      appendAlertLine(next, envelope.eventId, event.data);
      break;
    case "memory_worker":
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
    case "compact_done":
    case "compact_failed":
      return applyCompactionLifecycle(next, event.data.lifecycle);
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
        case "paste_artifact":
          return `[Large paste artifact ${segment.artifact.artifact_id}: ${segment.artifact.byte_len} bytes, ${segment.artifact.media_type}, ${segment.artifact.availability}, created ${segment.artifact.created_at_ms} ms, sha256 ${segment.artifact.sha256}]`;
        case "uploaded_file":
          return `[Attachment: ${segment.file.file_name} · ${segment.file.media_type} · ${segment.file.byte_len} bytes · ${segment.file.availability}]`;
        case "file_ref":
          return `@file ${segment.path}`;
        case "unknown":
          return "[unknown segment]";
      }
    })
    .join("\n");
}

function runStatsLine(
  eventId: string,
  stats: RunActivityStats,
  endedAtMs: number,
): ConsoleLine {
  const elapsedMs = endedAtMs - (stats.startedAtMs ?? endedAtMs);
  return line(
    eventId,
    "run_stats",
    "Run stats",
    `${formatRunElapsedCompact(elapsedMs)} ・${stats.requests} reqs ↑${
      formatRunTokens(stats.uploadTokens)
    }/↓${formatRunTokens(stats.outputTokens)}`,
  );
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

function systemItemLine(eventId: string, item: unknown): ConsoleLine {
  if (!isRecord(item)) {
    return line(eventId, "system", "System item", jsonPreview(item));
  }
  const itemKind = stringField(item, "kind") ?? "item";
  const title = `System · ${itemKind.replaceAll("_", " ")}`;
  const body = stringField(item, "body") ?? stringField(item, "message") ??
    stringField(item, "content") ?? jsonPreview(item);
  return {
    ...line(eventId, "system", title, body),
    systemItemKind: itemKind,
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
      cwd: update.cwd ?? projection.cwd,
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
      cwd: update.cwd ?? existing.toolCall!.cwd ?? projection.cwd,
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
      cwd: projection.cwd,
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
        cwd: projection.cwd,
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
  const commandTerminal = toolCall.command !== undefined &&
    toolCall.command.status !== "running";
  const commandError = toolCall.command !== undefined &&
    ["failed", "timed_out", "cancelled"].includes(toolCall.command.status);
  return {
    ...item,
    title: item.title.startsWith("Call · Tool result")
      ? item.title
      : `Call · ${toolCall.name}`,
    body: renderToolResponse(toolCall),
    expandedBody: renderToolResponse(toolCall, true),
    toolCallLabel: toolCallSignature(toolCall),
    toolStatus: toolCallStatus(toolCall),
    detail: toolCallDetail(toolCall),
    diff: toolCall.name === "Edit" ? editDiff(toolCall) : undefined,
    streaming: !["done", "error"].includes(toolCall.state) && !commandTerminal,
    error: toolCall.state === "error" || commandError,
  };
}

function renderToolResponse(toolCall: ToolCallView, expanded = false): string {
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
      return renderGrepTool(toolCall, expanded);
    case "Bash":
      return renderBashTool(toolCall, expanded);
    default:
      return renderDefaultTool(toolCall, expanded);
  }
}

function toolCallSignature(toolCall: ToolCallView): string {
  const args = parsedArgs(toolCall);
  switch (toolCall.name) {
    case "Read":
      return `Read(${readPath(toolCall)})`;
    case "Write":
    case "Edit": {
      const path = displayPath(stringField(args, "file_path") ?? "?", toolCall.cwd);
      return `${toolCall.name}(${path})`;
    }
    case "Glob":
      return `Glob(${stringField(args, "pattern") ?? genericCallArguments(toolCall)})`;
    case "Grep":
      return `Grep(${stringField(args, "pattern") ?? genericCallArguments(toolCall)})`;
    case "Bash": {
      const command = stringField(args, "command");
      return `Bash(${command ? `$ ${singleLine(command)}` : genericCallArguments(toolCall)})`;
    }
    default:
      return `${toolCall.name}(${genericCallArguments(toolCall)})`;
  }
}

function genericCallArguments(toolCall: ToolCallView): string {
  const raw = toolCall.arguments ?? toolCall.argsStream;
  if (!raw.trim()) return "";
  const parsed = parseJson(raw);
  if (parsed === undefined) return singleLine(raw);
  const serialized = JSON.stringify(parsed) ?? "null";
  return isRecord(parsed) ? serialized.slice(1, -1) : serialized;
}

function singleLine(value: string): string {
  return value.replace(/\s+/g, " ").trim();
}

function toolCallStatus(toolCall: ToolCallView): string {
  return toolCall.name === "Bash" ? commandStateSuffix(toolCall) : stateSuffix(toolCall.state);
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
    toolCallLabel: `Read(${count} file${plural(count)})`,
    toolStatus: hasError ? "failed" : inProgress ? "reading…" : "done",
    detail: calls.map(readDetail).join("\n\n"),
    eventId: group.at(-1)?.eventId,
    source: "event",
    streaming: inProgress,
    error: hasError,
    toolCall: {
      ...calls[0]!,
      state: hasError ? "error" : inProgress ? "running" : "done",
      isError: hasError,
    },
    toolCallCount: count,
  };
}

function readPath(toolCall: ToolCallView): string {
  return displayPath(readRawPath(toolCall), toolCall.cwd);
}

function readRawPath(toolCall: ToolCallView): string {
  const args = parsedArgs(toolCall);
  return stringField(args, "file_path") ?? "?";
}

function readDetail(toolCall: ToolCallView): string {
  return compactLines([
    `id: ${toolCall.id}`,
    `state: ${stateSuffix(toolCall.state)}`,
    `path: ${readPath(toolCall)}`,
    toolCall.summary
      ? `summary: ${
        normalizeKnownToolResult(toolCall.name, toolCall.summary, toolCall.cwd)
      }`
      : undefined,
  ]);
}

function renderReadTool(_toolCall: ToolCallView): string {
  return "";
}

function renderWriteTool(toolCall: ToolCallView): string {
  return knownToolResultText(toolCall) ?? "";
}

function renderEditTool(toolCall: ToolCallView): string {
  return knownToolResultText(toolCall) ?? "";
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
  return knownToolResultText(toolCall) ?? "";
}

function renderGrepTool(toolCall: ToolCallView, expanded: boolean): string {
  const result = knownToolResultText(toolCall);
  return expanded ? result ?? "" : cappedResultSection(result, 5) ?? "";
}

function renderBashTool(toolCall: ToolCallView, expanded: boolean): string {
  if (["done", "error"].includes(toolCall.state)) {
    const result = resultText(toolCall);
    return expanded ? result ?? "" : cappedDisplaySection(result, 10) ?? "";
  }
  return renderLiveCommandOutput(toolCall.command) ?? "";
}

function commandStateSuffix(toolCall: ToolCallView): string {
  const command = toolCall.command;
  if (!command) return stateSuffix(toolCall.state);
  if (command.status === "completed") {
    return command.exit_code === null
      ? "completed"
      : `completed (exit ${command.exit_code})`;
  }
  if (command.status === "failed") {
    return command.exit_code === null ? "failed" : `failed (exit ${command.exit_code})`;
  }
  if (command.status === "timed_out") return "timed out";
  if (command.status === "cancelled") return "cancelled";
  return "running…";
}

function commandTiming(command?: CommandSnapshot): string | undefined {
  if (!command) return undefined;
  const elapsed = Math.max(0, command.observed_at_ms - command.started_at_ms);
  if (command.status !== "running") return `elapsed ${durationLabel(elapsed)}`;
  if (command.last_output_at_ms === null) {
    return `elapsed ${durationLabel(elapsed)} · awaiting first output`;
  }
  const lastOutputElapsed = Math.max(
    0,
    command.last_output_at_ms - command.started_at_ms,
  );
  return `elapsed ${durationLabel(elapsed)} · last output at +${durationLabel(lastOutputElapsed)}`;
}

function durationLabel(milliseconds: number): string {
  if (milliseconds < 1000) return `${milliseconds}ms`;
  return `${(milliseconds / 1000).toFixed(milliseconds < 10_000 ? 1 : 0)}s`;
}

function renderLiveCommandOutput(command?: CommandSnapshot): string | undefined {
  if (!command) return undefined;
  const stdout = compactLines([
    command.stdout.truncated ? "[… earlier stdout omitted]" : undefined,
    command.stdout.content,
  ]);
  const stderr = compactLines([
    command.stderr.truncated ? "[… earlier stderr omitted]" : undefined,
    command.stderr.content,
  ]);
  return compactLines([
    stdout,
    stderr ? `stderr:\n${stderr}` : undefined,
  ]);
}

function renderDefaultTool(toolCall: ToolCallView, expanded: boolean): string {
  const result = resultText(toolCall);
  return expanded ? result ?? "" : cappedDisplaySection(result, 3) ?? "";
}

function toolCallDetail(toolCall: ToolCallView): string {
  return compactLines([
    `id: ${toolCall.id}`,
    `state: ${stateSuffix(toolCall.state)}`,
    toolCall.command ? `command: ${commandTiming(toolCall.command)}` : undefined,
    toolCall.summary
      ? `summary: ${
        normalizeKnownToolResult(toolCall.name, toolCall.summary, toolCall.cwd)
      }`
      : undefined,
    argsText(toolCall) ? `arguments:\n${argsText(toolCall)}` : undefined,
  ]);
}

function resultText(toolCall: ToolCallView): string | undefined {
  const text = toolCall.output || toolCall.summary;
  return text ? formatJsonResponseAsYaml(text) : undefined;
}

function formatJsonResponseAsYaml(text: string): string {
  const trimmed = text.trim();
  if (
    !(
      (trimmed.startsWith("{") && trimmed.endsWith("}")) ||
      (trimmed.startsWith("[") && trimmed.endsWith("]"))
    )
  ) {
    return text;
  }

  try {
    const parsed: unknown = JSON.parse(trimmed);
    if (parsed === null || typeof parsed !== "object") {
      return text;
    }
    return stringifyYaml(parsed).trimEnd();
  } catch {
    return text;
  }
}

function knownToolResultText(toolCall: ToolCallView): string | undefined {
  const text = resultText(toolCall);
  if (!text) return undefined;
  return normalizeKnownToolResult(toolCall.name, text, toolCall.cwd);
}

function normalizeKnownToolResult(
  toolName: string,
  text: string,
  cwd: string | null | undefined,
): string {
  if (!cwd) return text;
  switch (toolName) {
    case "Read":
      return normalizeReadResultPaths(text, cwd);
    case "Grep":
      return normalizeLineStartPaths(text, cwd);
    case "Glob":
      return normalizeLineStartPaths(text, cwd);
    case "Edit":
    case "Write":
      return normalizeEditResultPaths(text, cwd);
    default:
      return text;
  }
}

function displayPath(path: string, cwd: string | null | undefined): string {
  if (!cwd || !path.startsWith("/")) return path;
  const normalizedCwd = cwd.endsWith("/") ? cwd.slice(0, -1) : cwd;
  if (path === normalizedCwd) return ".";
  const prefix = `${normalizedCwd}/`;
  return path.startsWith(prefix) ? path.slice(prefix.length) : path;
}

function normalizeReadResultPaths(text: string, cwd: string): string {
  return text.split("\n").map((line) => {
    const readMatch = line.match(
      /^(Read \d+ line\(s\)(?: \[[^\]]+\])?(?: of \d+)? from )(.+)$/,
    );
    if (readMatch) {
      return `${readMatch[1]}${displayPath(readMatch[2], cwd)}`;
    }
    return normalizeKnownPathSuffix(line, cwd);
  }).join("\n");
}

function normalizeEditResultPaths(text: string, cwd: string): string {
  return text.split("\n").map((line) => {
    const simpleMatch = line.match(
      /^(Edited |Wrote |Created |Overwrote |Deleted )(.+?)( \(.+\))?$/,
    );
    if (simpleMatch) {
      return `${simpleMatch[1]}${displayPath(simpleMatch[2], cwd)}${
        simpleMatch[3] ?? ""
      }`;
    }
    return normalizeKnownPathSuffix(line, cwd);
  }).join("\n");
}

function normalizeKnownPathSuffix(line: string, cwd: string): string {
  const match = line.match(
    /^(.*\b(?:from|path|file not found|not a directory): )(.+)$/,
  );
  if (!match) return line;
  return `${match[1]}${displayPath(match[2], cwd)}`;
}

function normalizeLineStartPaths(text: string, cwd: string): string {
  const normalizedCwd = cwd.endsWith("/") ? cwd.slice(0, -1) : cwd;
  const prefix = `${normalizedCwd}/`;
  return text.split("\n").map((line) => {
    if (line === normalizedCwd) return ".";
    return line.startsWith(prefix) ? line.slice(prefix.length) : line;
  }).join("\n");
}

function argsText(toolCall: ToolCallView): string {
  const raw = toolCall.arguments ?? toolCall.argsStream;
  if (!raw.trim()) {
    return "";
  }
  const parsed = parseJson(raw);
  return parsed === undefined ? raw : stringifyYaml(parsed).trimEnd();
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

function applyTaskTool(
  projection: ConsoleProjection,
  name: string,
  argumentsJson: string,
): void {
  const state = applyTaskToolCall(
    { tasks: projection.tasks, nextTaskId: projection.taskNextId },
    name,
    argumentsJson,
  );
  projection.tasks = state.tasks;
  projection.taskNextId = state.nextTaskId;
}

function applyTaskSnapshot(projection: ConsoleProjection, text: string): void {
  const state = applyTaskSnapshotText(
    { tasks: projection.tasks, nextTaskId: projection.taskNextId },
    text,
  );
  projection.tasks = state.tasks;
  projection.taskNextId = state.nextTaskId;
}

function applyTaskSystemItem(
  projection: ConsoleProjection,
  item: unknown,
): void {
  if (!isRecord(item)) return;
  const body = item["body"];
  if (typeof body === "string") applyTaskSnapshot(projection, body);
}

function snapshotProjectionFromSession(
  eventId: string,
  snapshot: unknown,
  cwd: string | null,
): ConsoleProjection {
  const projection: ConsoleProjection = {
    lines: [],
    tasks: [],
    taskNextId: 1,
    status: null,
    usage: null,
    runActivity: emptyRunActivityStats(),
    cwd,
    lastEventId: eventId,
    internalWorkers: [],
    removedInternalWorkers: {},
  };
  const entries = isRecord(snapshot) ? arrayField(snapshot, "entries") : [];
  entries.forEach((entry, index) =>
    applySessionEntry(projection, `${eventId}-snapshot-${index}`, entry)
  );
  return projection;
}

function applySessionEntry(
  projection: ConsoleProjection,
  fallbackEventId: string,
  value: unknown,
): void {
  if (!isRecord(value)) return;
  const eventId = stringField(value, "entry_id") ?? fallbackEventId;
  switch (stringField(value, "kind")) {
    case "user_input":
      projection.lines.push(
        line(
          eventId,
          "user",
          "User",
          segmentsToText(arrayField(value, "segments") as Segment[]),
        ),
      );
      break;
    case "message":
      applyLoggedItem(projection, eventId, {
        kind: "message",
        role: value["role"],
        content: value["content"],
      });
      break;
    case "tool_call":
      applyLoggedItem(projection, eventId, {
        kind: "tool_call",
        call_id: value["call_id"],
        name: value["name"],
        arguments: value["arguments"],
      });
      break;
    case "tool_result":
      applyLoggedItem(projection, eventId, {
        kind: "tool_result",
        call_id: value["call_id"],
        summary: value["summary"],
        content: value["content"],
        is_error: value["is_error"],
      });
      break;
    case "system_item": {
      const item = value["data"];
      projection.lines.push(systemItemLine(eventId, item ?? value));
      applyTaskSystemItem(projection, item);
      break;
    }
    case "run_error":
      projection.lines.push(
        line(
          eventId,
          "error",
          "Run error",
          stringField(value, "message") ?? "Worker run failed.",
          undefined,
          false,
          true,
        ),
      );
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
  if (!isRecord(payload)) return;
  if (
    typeof payload["compaction_id"] === "string" &&
    typeof payload["revision"] === "number" &&
    typeof payload["state"] === "string" &&
    typeof payload["started_at_ms"] === "number"
  ) {
    const updated = applyCompactionLifecycle(
      projection,
      payload as unknown as CompactionLifecycle,
    );
    projection.lines = updated.lines;
    return;
  }
  // Schema v1 remains readable historical evidence.
  if (payload["kind"] !== "compaction_block") {
    return;
  }
  const blockId = stringField(payload, "block_id") || "compact";
  const state = stringField(payload, "state") || "running";
  const message = stringField(payload, "message") ||
    compactMessageForState(state, payload);
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

function applyLoggedUserInput(
  projection: ConsoleProjection,
  eventId: string,
  entry: Record<string, unknown>,
): void {
  let body = segmentsToText(arrayField(entry, "segments") as Segment[]);
  if (!body && stringField(entry, "kind") === "annotated_user_input") {
    body = loggedUserText(arrayField(entry, "history"));
  }
  projection.lines.push(line(eventId, "user", "User", body));
}

function loggedUserText(history: unknown[]): string {
  for (const historyEntry of history) {
    if (!isRecord(historyEntry) || !isRecord(historyEntry["item"])) continue;
    const item = historyEntry["item"];
    if (
      stringField(item, "kind") !== "message" ||
      stringField(item, "role") !== "user"
    ) {
      continue;
    }
    return loggedContentText(arrayField(item, "content"));
  }
  return "";
}

function applyLoggedHistoryEntry(
  projection: ConsoleProjection,
  eventId: string,
  historyEntry: unknown,
): void {
  if (!isRecord(historyEntry)) return;
  applyLoggedItem(projection, eventId, historyEntry["item"]);
}

function applyLoggedSystemEntry(
  projection: ConsoleProjection,
  eventId: string,
  historyEntry: unknown,
): void {
  if (!isRecord(historyEntry)) return;
  projection.lines.push(systemItemLine(eventId, historyEntry["item"]));
  applyTaskSystemItem(projection, historyEntry["item"]);
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
        case "system":
          applyTaskSnapshot(projection, body);
          break;
        default:
          break;
      }
      break;
    }
    case "reasoning": {
      const text = stringField(item, "text") ??
        arrayField(item, "summary").filter((value) => typeof value === "string")
          .join("\n");
      if (text) {
        projection.lines.push(line(eventId, "thinking", "Thought", text));
      }
      break;
    }
    case "tool_call": {
      const name = stringField(item, "name") ?? "Tool";
      const argumentsJson = stringField(item, "arguments") ?? "";
      upsertToolCall(
        projection,
        eventId,
        stringField(item, "call_id") ?? eventId,
        {
          name,
          arguments: argumentsJson,
          argsStream: argumentsJson,
          state: "running",
        },
      );
      applyTaskTool(projection, name, argumentsJson);
      break;
    }
    case "tool_result":
      attachToolResult(
        projection,
        eventId,
        stringField(item, "call_id") ?? eventId,
        {
          summary: stringField(item, "summary") ?? "",
          output: stringField(item, "content"),
          isError: item["is_error"] === true,
        },
      );
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

function inFlightLine(
  eventId: string,
  block: InFlightBlock,
  cwd: string | null,
): ConsoleLine {
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
        cwd,
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
