import type {
  BrowserCreateWorkerResponse,
  BrowserWorkerWorkingDirectorySelection,
  BrowserWorkspaceOrchestratorResponse,
  CreateWorkspaceWorkerRequest,
  CreateWorkspaceWorkerTicketAssignmentRequest,
  Diagnostic,
  DiagnosticSeverity,
  RuntimeWorkingDirectoryCleanupTarget,
  RuntimeWorkingDirectorySummary,
  WorkerCapabilitySummary,
  WorkerImplementationSummary,
  WorkerLaunchOptionsResponse,
  WorkerLaunchProfileCandidate,
  WorkerLaunchRuntimeOption,
  WorkerLaunchWorkerSummary,
  WorkerWorkspaceSummary,
  WorkingDirectoryRepositoryOption,
} from "$lib/generated/worker-launch-api";
import type { Segment } from "$lib/generated/protocol";
import { parseWorkingDirectorySummary } from "$lib/workspace/api/workdirs";

const DIAGNOSTIC_SEVERITIES = new Set<DiagnosticSeverity>([
  "info",
  "warning",
  "error",
]);

function record(value: unknown, label: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
  return value as Record<string, unknown>;
}

function exact(
  value: Record<string, unknown>,
  allowed: readonly string[],
  label: string,
): void {
  const unexpected = Object.keys(value).filter((key) => !allowed.includes(key));
  if (unexpected.length > 0) {
    throw new Error(`${label} contains unknown field ${unexpected[0]}`);
  }
}

function string(value: unknown, label: string): string {
  if (typeof value !== "string") throw new Error(`${label} must be a string`);
  return value;
}

function boolean(value: unknown, label: string): boolean {
  if (typeof value !== "boolean") throw new Error(`${label} must be a boolean`);
  return value;
}

function number(value: unknown, label: string): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new Error(`${label} must be a finite number`);
  }
  return value;
}

function nullableString(value: unknown, label: string): string | null {
  return value === null ? null : string(value, label);
}

function array<T>(
  value: unknown,
  label: string,
  parse: (item: unknown, label: string) => T,
): T[] {
  if (!Array.isArray(value)) throw new Error(`${label} must be an array`);
  return value.map((item, index) => parse(item, `${label}[${index}]`));
}

function optional<T>(
  value: unknown,
  label: string,
  parse: (item: unknown, label: string) => T,
): T | null | undefined {
  return value === undefined
    ? undefined
    : value === null
    ? null
    : parse(value, label);
}

function diagnostic(value: unknown, label: string): Diagnostic {
  const item = record(value, label);
  exact(item, ["code", "severity", "message"], label);
  const severity = string(item.severity, `${label}.severity`);
  if (!DIAGNOSTIC_SEVERITIES.has(severity as DiagnosticSeverity)) {
    throw new Error(`${label}.severity is invalid`);
  }
  return {
    code: string(item.code, `${label}.code`),
    severity: severity as DiagnosticSeverity,
    message: string(item.message, `${label}.message`),
  };
}

function runtimeOption(
  value: unknown,
  label: string,
): WorkerLaunchRuntimeOption {
  const item = record(value, label);
  exact(
    item,
    [
      "runtime_id",
      "display_name",
      "built_in",
      "worker_creation_available",
      "working_directory_required",
      "status",
      "diagnostics",
    ],
    label,
  );
  return {
    runtime_id: string(item.runtime_id, `${label}.runtime_id`),
    display_name: string(item.display_name, `${label}.display_name`),
    built_in: boolean(item.built_in, `${label}.built_in`),
    worker_creation_available: boolean(
      item.worker_creation_available,
      `${label}.worker_creation_available`,
    ),
    working_directory_required: boolean(
      item.working_directory_required,
      `${label}.working_directory_required`,
    ),
    status: string(item.status, `${label}.status`),
    diagnostics: array(item.diagnostics, `${label}.diagnostics`, diagnostic),
  };
}

function profileCandidate(
  value: unknown,
  label: string,
): WorkerLaunchProfileCandidate {
  const item = record(value, label);
  exact(item, ["id", "label", "description"], label);
  return {
    id: string(item.id, `${label}.id`),
    label: string(item.label, `${label}.label`),
    description: string(item.description, `${label}.description`),
  };
}

function repositoryOption(
  value: unknown,
  label: string,
): WorkingDirectoryRepositoryOption {
  const item = record(value, label);
  exact(item, ["repository_key", "default_selector"], label);
  return {
    repository_key: string(item.repository_key, `${label}.repository_key`),
    default_selector: optional(
      item.default_selector,
      `${label}.default_selector`,
      string,
    ),
  };
}

export function parseWorkerLaunchOptionsResponse(
  value: unknown,
): WorkerLaunchOptionsResponse {
  const item = record(value, "Worker launch options response");
  exact(
    item,
    [
      "workspace_id",
      "runtimes",
      "default_profile",
      "profiles",
      "repositories",
      "working_directories",
      "diagnostics",
    ],
    "Worker launch options response",
  );
  return {
    workspace_id: string(item.workspace_id, "workspace_id"),
    runtimes: array(item.runtimes, "runtimes", runtimeOption),
    default_profile: nullableString(item.default_profile, "default_profile"),
    profiles: array(item.profiles, "profiles", profileCandidate),
    repositories: array(item.repositories, "repositories", repositoryOption),
    working_directories: array(
      item.working_directories,
      "working_directories",
      parseWorkingDirectorySummary,
    ),
    diagnostics: array(item.diagnostics, "diagnostics", diagnostic),
  };
}

function workspaceSummary(
  value: unknown,
  label: string,
): WorkerWorkspaceSummary {
  const item = record(value, label);
  exact(item, ["visibility", "identity", "workspace_id"], label);
  return {
    visibility: string(item.visibility, `${label}.visibility`),
    identity: string(item.identity, `${label}.identity`),
    workspace_id: optional(item.workspace_id, `${label}.workspace_id`, string),
  };
}

function implementationSummary(
  value: unknown,
  label: string,
): WorkerImplementationSummary {
  const item = record(value, label);
  exact(item, ["kind", "display_hint"], label);
  return {
    kind: string(item.kind, `${label}.kind`),
    display_hint: string(item.display_hint, `${label}.display_hint`),
  };
}

function capabilitySummary(
  value: unknown,
  label: string,
): WorkerCapabilitySummary {
  const item = record(value, label);
  exact(item, ["can_stop", "can_spawn_followup"], label);
  return {
    can_stop: boolean(item.can_stop, `${label}.can_stop`),
    can_spawn_followup: boolean(
      item.can_spawn_followup,
      `${label}.can_spawn_followup`,
    ),
  };
}

function runtimeCleanupTarget(
  value: unknown,
  label: string,
): RuntimeWorkingDirectoryCleanupTarget {
  const item = record(value, label);
  exact(item, ["kind", "working_directory_id", "repository_id"], label);
  return {
    kind: string(item.kind, `${label}.kind`),
    working_directory_id: string(
      item.working_directory_id,
      `${label}.working_directory_id`,
    ),
    repository_id: string(item.repository_id, `${label}.repository_id`),
  };
}

function runtimeWorkingDirectory(
  value: unknown,
  label: string,
): RuntimeWorkingDirectorySummary {
  const item = record(value, label);
  exact(
    item,
    [
      "working_directory_id",
      "repository_id",
      "creation_selector",
      "creation_ref",
      "creation_tree",
      "current_selector",
      "current_ref",
      "current_tree",
      "observed_at_epoch_seconds",
      "materializer_kind",
      "cleanup_target",
      "status",
      "cleanliness",
      "primary_worker_id",
      "occupied_by",
    ],
    label,
  );
  const materializerKind = string(
    item.materializer_kind,
    `${label}.materializer_kind`,
  );
  if (
    materializerKind !== "runtime_git_cache" &&
    materializerKind !== "local_git_worktree"
  ) {
    throw new Error(`${label}.materializer_kind is invalid`);
  }
  const status = string(item.status, `${label}.status`);
  if (
    !["active", "cleanup_pending", "corrupted", "not_found", "unknown"]
      .includes(status)
  ) {
    throw new Error(`${label}.status is invalid`);
  }
  const occupied = optional(
    item.occupied_by,
    `${label}.occupied_by`,
    (value, occupiedLabel) => {
      const occupancy = record(value, occupiedLabel);
      exact(
        occupancy,
        ["runtime_id", "worker_id", "display_name", "linked_at"],
        occupiedLabel,
      );
      return {
        runtime_id: string(occupancy.runtime_id, `${occupiedLabel}.runtime_id`),
        worker_id: string(occupancy.worker_id, `${occupiedLabel}.worker_id`),
        display_name: string(
          occupancy.display_name,
          `${occupiedLabel}.display_name`,
        ),
        linked_at: string(occupancy.linked_at, `${occupiedLabel}.linked_at`),
      };
    },
  );
  return {
    working_directory_id: string(
      item.working_directory_id,
      `${label}.working_directory_id`,
    ),
    repository_id: string(item.repository_id, `${label}.repository_id`),
    creation_selector: optional(
      item.creation_selector,
      `${label}.creation_selector`,
      string,
    ),
    creation_ref: optional(item.creation_ref, `${label}.creation_ref`, string),
    creation_tree: optional(
      item.creation_tree,
      `${label}.creation_tree`,
      string,
    ),
    current_selector: optional(
      item.current_selector,
      `${label}.current_selector`,
      string,
    ),
    current_ref: optional(item.current_ref, `${label}.current_ref`, string),
    current_tree: optional(item.current_tree, `${label}.current_tree`, string),
    observed_at_epoch_seconds: optional(
      item.observed_at_epoch_seconds,
      `${label}.observed_at_epoch_seconds`,
      number,
    ),
    materializer_kind: materializerKind,
    cleanup_target: optional(
      item.cleanup_target,
      `${label}.cleanup_target`,
      runtimeCleanupTarget,
    ),
    status: status as RuntimeWorkingDirectorySummary["status"],
    cleanliness: optional(item.cleanliness, `${label}.cleanliness`, string),
    primary_worker_id: optional(
      item.primary_worker_id,
      `${label}.primary_worker_id`,
      string,
    ),
    occupied_by: occupied,
  };
}

function workerSummary(
  value: unknown,
  label: string,
): WorkerLaunchWorkerSummary {
  const item = record(value, label);
  exact(
    item,
    [
      "runtime_id",
      "worker_id",
      "host_id",
      "display_name",
      "label",
      "profile",
      "singleton_key",
      "tags",
      "workspace",
      "state",
      "last_seen_at",
      "pinned",
      "retention_state",
      "implementation",
      "capabilities",
      "working_directory",
      "diagnostics",
    ],
    label,
  );
  return {
    runtime_id: string(item.runtime_id, `${label}.runtime_id`),
    worker_id: string(item.worker_id, `${label}.worker_id`),
    host_id: string(item.host_id, `${label}.host_id`),
    display_name: string(item.display_name, `${label}.display_name`),
    label: string(item.label, `${label}.label`),
    profile: nullableString(item.profile, `${label}.profile`),
    singleton_key: nullableString(item.singleton_key, `${label}.singleton_key`),
    tags: array(item.tags, `${label}.tags`, string),
    workspace: workspaceSummary(item.workspace, `${label}.workspace`),
    state: string(item.state, `${label}.state`),
    last_seen_at: nullableString(item.last_seen_at, `${label}.last_seen_at`),
    pinned: boolean(item.pinned, `${label}.pinned`),
    retention_state: string(item.retention_state, `${label}.retention_state`),
    implementation: implementationSummary(
      item.implementation,
      `${label}.implementation`,
    ),
    capabilities: capabilitySummary(item.capabilities, `${label}.capabilities`),
    working_directory: optional(
      item.working_directory,
      `${label}.working_directory`,
      runtimeWorkingDirectory,
    ),
    diagnostics: array(item.diagnostics, `${label}.diagnostics`, diagnostic),
  };
}

export function parseBrowserCreateWorkerResponse(
  value: unknown,
): BrowserCreateWorkerResponse {
  const item = record(value, "Worker create response");
  exact(
    item,
    [
      "workspace_id",
      "runtime_id",
      "worker_id",
      "console_href",
      "worker",
      "diagnostics",
    ],
    "Worker create response",
  );
  return {
    workspace_id: string(item.workspace_id, "workspace_id"),
    runtime_id: string(item.runtime_id, "runtime_id"),
    worker_id: string(item.worker_id, "worker_id"),
    console_href: string(item.console_href, "console_href"),
    worker: workerSummary(item.worker, "worker"),
    diagnostics: array(item.diagnostics, "diagnostics", diagnostic),
  };
}

export function parseBrowserWorkspaceOrchestratorResponse(
  value: unknown,
): BrowserWorkspaceOrchestratorResponse {
  const item = record(value, "Workspace Orchestrator response");
  exact(
    item,
    ["workspace_id", "online", "disposition", "worker", "diagnostics"],
    "Workspace Orchestrator response",
  );
  return {
    workspace_id: string(item.workspace_id, "workspace_id"),
    online: boolean(item.online, "online"),
    disposition: string(item.disposition, "disposition"),
    worker: optional(item.worker, "worker", workerSummary),
    diagnostics: array(item.diagnostics, "diagnostics", diagnostic),
  };
}

function workingDirectorySelection(
  value: unknown,
  label: string,
): BrowserWorkerWorkingDirectorySelection {
  const item = record(value, label);
  exact(item, ["working_directory_id", "relative_cwd"], label);
  return {
    working_directory_id: string(
      item.working_directory_id,
      `${label}.working_directory_id`,
    ),
    relative_cwd: nullableString(item.relative_cwd, `${label}.relative_cwd`),
  };
}

function ticketAssignment(
  value: unknown,
  label: string,
): CreateWorkspaceWorkerTicketAssignmentRequest {
  const item = record(value, label);
  exact(item, ["ticket_id", "operation_id"], label);
  return {
    ticket_id: string(item.ticket_id, `${label}.ticket_id`),
    operation_id: string(item.operation_id, `${label}.operation_id`),
  };
}

function unsignedInteger(value: unknown, label: string): number {
  const parsed = number(value, label);
  if (!Number.isSafeInteger(parsed) || parsed < 0) {
    throw new Error(`${label} must be a non-negative safe integer`);
  }
  return parsed;
}

function pasteArtifact(
  value: unknown,
  label: string,
): Extract<Segment, { kind: "paste_artifact" }>["artifact"] {
  const item = record(value, label);
  exact(
    item,
    [
      "artifact_id",
      "created_at_ms",
      "media_type",
      "availability",
      "byte_len",
      "char_count",
      "line_count",
      "sha256",
      "source_entry_id",
    ],
    label,
  );
  const mediaType = string(item.media_type, `${label}.media_type`);
  if (mediaType !== "text_plain_utf8") {
    throw new Error(`${label}.media_type is invalid`);
  }
  const availability = string(item.availability, `${label}.availability`);
  if (
    !["available", "unavailable", "integrity_failed"].includes(availability)
  ) {
    throw new Error(`${label}.availability is invalid`);
  }
  return {
    artifact_id: string(item.artifact_id, `${label}.artifact_id`),
    created_at_ms: unsignedInteger(
      item.created_at_ms,
      `${label}.created_at_ms`,
    ),
    media_type: mediaType,
    availability: availability as Extract<Segment, { kind: "paste_artifact" }>[
      "artifact"
    ]["availability"],
    byte_len: unsignedInteger(item.byte_len, `${label}.byte_len`),
    char_count: unsignedInteger(item.char_count, `${label}.char_count`),
    line_count: unsignedInteger(item.line_count, `${label}.line_count`),
    sha256: string(item.sha256, `${label}.sha256`),
    source_entry_id: string(item.source_entry_id, `${label}.source_entry_id`),
  };
}

function uploadedFile(
  value: unknown,
  label: string,
): Extract<Segment, { kind: "uploaded_file" }>["file"] {
  const item = record(value, label);
  exact(
    item,
    [
      "artifact_id",
      "file_name",
      "media_type",
      "created_at_ms",
      "availability",
      "byte_len",
      "sha256",
      "source_entry_id",
    ],
    label,
  );
  const availability = string(item.availability, `${label}.availability`);
  if (
    !["available", "unavailable", "integrity_failed"].includes(availability)
  ) {
    throw new Error(`${label}.availability is invalid`);
  }
  return {
    artifact_id: string(item.artifact_id, `${label}.artifact_id`),
    file_name: string(item.file_name, `${label}.file_name`),
    media_type: string(item.media_type, `${label}.media_type`),
    created_at_ms: unsignedInteger(
      item.created_at_ms,
      `${label}.created_at_ms`,
    ),
    availability: availability as Extract<Segment, { kind: "uploaded_file" }>[
      "file"
    ]["availability"],
    byte_len: unsignedInteger(item.byte_len, `${label}.byte_len`),
    sha256: string(item.sha256, `${label}.sha256`),
    source_entry_id: optional(
      item.source_entry_id,
      `${label}.source_entry_id`,
      string,
    ),
  };
}

function segment(value: unknown, label: string): Segment {
  const item = record(value, label);
  const kind = string(item.kind, `${label}.kind`) as Segment["kind"];
  switch (kind) {
    case "text":
      exact(item, ["kind", "content"], label);
      return { kind, content: string(item.content, `${label}.content`) };
    case "paste":
      exact(item, ["kind", "id", "chars", "lines", "content"], label);
      return {
        kind,
        id: unsignedInteger(item.id, `${label}.id`),
        chars: unsignedInteger(item.chars, `${label}.chars`),
        lines: unsignedInteger(item.lines, `${label}.lines`),
        content: string(item.content, `${label}.content`),
      };
    case "paste_artifact":
      exact(item, ["kind", "artifact"], label);
      return {
        kind,
        artifact: pasteArtifact(item.artifact, `${label}.artifact`),
      };
    case "uploaded_file":
      exact(item, ["kind", "file"], label);
      return { kind, file: uploadedFile(item.file, `${label}.file`) };
    case "file_ref":
      exact(item, ["kind", "path"], label);
      return { kind, path: string(item.path, `${label}.path`) };
    case "flow":
      exact(item, ["kind", "selector"], label);
      return { kind, selector: string(item.selector, `${label}.selector`) };
    case "unknown":
      throw new Error(`${label}.kind is not supported by Worker creation`);
  }
  const exhaustive: never = kind;
  throw new Error(`${label}.kind is invalid: ${exhaustive}`);
}

export function parseCreateWorkspaceWorkerRequest(
  value: unknown,
): CreateWorkspaceWorkerRequest {
  const item = record(value, "Worker create request");
  exact(
    item,
    [
      "runtime_id",
      "display_name",
      "profile",
      "ticket_assignment",
      "initial_submit",
      "working_directory",
      "control_operation_id",
    ],
    "Worker create request",
  );
  return {
    runtime_id: string(item.runtime_id, "runtime_id"),
    display_name: string(item.display_name, "display_name"),
    profile: nullableString(item.profile, "profile"),
    ticket_assignment: item.ticket_assignment === null
      ? null
      : ticketAssignment(item.ticket_assignment, "ticket_assignment"),
    initial_submit: array(item.initial_submit, "initial_submit", segment),
    working_directory: item.working_directory === null
      ? null
      : workingDirectorySelection(item.working_directory, "working_directory"),
    control_operation_id: nullableString(
      item.control_operation_id,
      "control_operation_id",
    ),
  };
}
