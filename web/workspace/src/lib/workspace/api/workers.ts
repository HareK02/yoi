import type {
  BrowserCreateWorkerResponse,
  BrowserWorkerWorkingDirectorySelection,
  BrowserWorkspaceOrchestratorResponse,
  CreateWorkspaceWorkerRequest,
  Diagnostic,
  DiagnosticSeverity,
  RepositoryApiError,
  RuntimeWorkerWorkdirAttachmentSummary,
  RuntimeWorkingDirectoryCleanupTarget,
  RuntimeWorkingDirectorySummary,
  WorkerCreationAuthorization,
  WorkerImplementationSummary,
  WorkerLaunchOptionsResponse,
  WorkerLaunchProfileCandidate,
  WorkerLaunchRuntimeOption,
  WorkerLaunchWorkerSummary,
  WorkerLifecycleState,
  WorkerListResponse,
  WorkerRemovalCause,
  WorkerRemovalObservation,
  WorkerRemovalPhase,
  WorkerRemovalState,
  WorkerStateSnapshot,
  WorkerSummary,
  WorkerWorkdirAttachmentSummary,
  WorkerWorkspaceSummary,
  WorkingDirectoryRepositoryOption,
} from "#lib/generated/worker-launch-api.ts";
import { parseAssignment } from "#lib/workspace/memory/permissions.ts";
import type { Segment } from "#lib/generated/protocol.ts";
import {
  MAX_FEATURE_INITIALIZATIONS,
  MAX_FEATURE_INITIALIZATION_BYTES,
  MAX_FEATURE_INITIALIZATION_ID_BYTES,
  MAX_FEATURE_INITIALIZATION_PAYLOAD_BYTES,
} from "#lib/generated/protocol.ts";
import {
  parseExternalWorkdirPermissions,
  parseWorkingDirectorySummary,
} from "#lib/workspace/api/workdirs.ts";

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

const MAX_STRING_LENGTH = 65_536;
const MAX_COLLECTION_LENGTH = 4_096;

function string(value: unknown, label: string): string {
  if (typeof value !== "string") throw new Error(`${label} must be a string`);
  if (value.length > MAX_STRING_LENGTH) {
    throw new Error(`${label} exceeds its length limit`);
  }
  return value;
}

function boolean(value: unknown, label: string): boolean {
  if (typeof value !== "boolean") throw new Error(`${label} must be a boolean`);
  return value;
}

function number(value: unknown, label: string): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value)) {
    throw new Error(`${label} must be a safe integer`);
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
  if (value.length > MAX_COLLECTION_LENGTH) {
    throw new Error(`${label} exceeds its item limit`);
  }
  return value.map((item, index) => parse(item, `${label}[${index}]`));
}

function optional<T>(
  value: unknown,
  label: string,
  parse: (item: unknown, label: string) => T,
): T | null | undefined {
  return value === undefined ? undefined : value === null ? null : parse(value, label);
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
      "supports_workdir_attachments",
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
    supports_workdir_attachments: boolean(
      item.supports_workdir_attachments,
      `${label}.supports_workdir_attachments`,
    ),
    status: string(item.status, `${label}.status`),
    diagnostics: array(item.diagnostics, `${label}.diagnostics`, diagnostic),
  };
}

function featureConnectionOptions(value: unknown, label: string) {
  const item = record(value, label);
  exact(item, ["subjektiv"], label);
  return { subjektiv: boolean(item.subjektiv, `${label}.subjektiv`) };
}

function profileCandidate(
  value: unknown,
  label: string,
): WorkerLaunchProfileCandidate {
  const item = record(value, label);
  exact(item, ["id", "label", "description", "feature_connections"], label);
  return {
    id: string(item.id, `${label}.id`),
    label: string(item.label, `${label}.label`),
    description: string(item.description, `${label}.description`),
    feature_connections: featureConnectionOptions(
      item.feature_connections,
      `${label}.feature_connections`,
    ),
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

export function parseWorkerApiError(value: unknown): RepositoryApiError {
  const item = record(value, "Worker API error");
  exact(item, ["error", "message", "diagnostics"], "Worker API error");
  return {
    error: string(item.error, "error"),
    message: string(item.message, "message"),
    diagnostics: item.diagnostics === undefined
      ? []
      : array(item.diagnostics, "diagnostics", diagnostic),
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

function workerStateSnapshot(
  value: unknown,
  label: string,
): WorkerStateSnapshot {
  const snapshot = record(value, label);
  exact(
    snapshot,
    ["state_sequence", "last_command_id", "last_finished_submission_request_id", "state"],
    label,
  );
  const lastFinishedSubmissionRequestId = optional(
    snapshot.last_finished_submission_request_id,
    `${label}.last_finished_submission_request_id`,
    string,
  );
  const state = record(snapshot.state, `${label}.state`);
  const kind = string(state.kind, `${label}.state.kind`);
  if (kind === "idle") {
    exact(state, ["kind"], `${label}.state`);
    return {
      state_sequence: unsignedInteger(snapshot.state_sequence, `${label}.state_sequence`),
      last_command_id: unsignedInteger(
        snapshot.last_command_id,
        `${label}.last_command_id`,
      ),
      last_finished_submission_request_id: lastFinishedSubmissionRequestId,
      state: { kind: "idle" },
    };
  }
  if (kind !== "busy") throw new Error(`${label}.state.kind is invalid`);
  exact(state, ["kind", "state"], `${label}.state`);
  const busy = record(state.state, `${label}.state.state`);
  exact(busy, ["kind", "state"], `${label}.state.state`);
  const busyKind = string(busy.kind, `${label}.state.state.kind`);
  const busyState = string(busy.state, `${label}.state.state.state`);
  if (
    (busyKind === "run" &&
      ["running", "pausing", "paused", "cancelling"].includes(busyState)) ||
    (busyKind === "maintenance" && busyState === "compacting")
  ) {
    return {
      state_sequence: unsignedInteger(snapshot.state_sequence, `${label}.state_sequence`),
      last_command_id: unsignedInteger(
        snapshot.last_command_id,
        `${label}.last_command_id`,
      ),
      last_finished_submission_request_id: lastFinishedSubmissionRequestId,
      state: {
        kind: "busy",
        state: busyKind === "run"
          ? {
            kind: "run",
            state: busyState as "running" | "pausing" | "paused" | "cancelling",
          }
          : { kind: "maintenance", state: "compacting" },
      },
    };
  }
  throw new Error(`${label}.state.state is invalid`);
}

function workerWorkdirAttachment(
  value: unknown,
  label: string,
): WorkerWorkdirAttachmentSummary {
  const item = record(value, label);
  exact(item, ["alias", "effective_permissions", "working_directory"], label);
  return {
    alias: string(item.alias, `${label}.alias`),
    effective_permissions: parseExternalWorkdirPermissions(
      item.effective_permissions,
      `${label}.effective_permissions`,
    ),
    working_directory: parseWorkingDirectorySummary(item.working_directory),
  };
}

export type ParsedWorkerSummary =
  & Omit<
    WorkerSummary,
    "display_name" | "tags" | "diagnostics"
  >
  & {
    display_name: string;
    tags: string[];
    diagnostics: Diagnostic[];
  };

export type ParsedWorkerListResponse = Omit<WorkerListResponse, "items"> & {
  items: ParsedWorkerSummary[];
};

const WORKER_LIFECYCLE_STATES: readonly WorkerLifecycleState[] = [
  "not_started",
  "awaiting_restore",
  "active",
  "stopped",
  "creating",
  "restoring",
  "stopping",
  "recovery_required",
];

function workerLifecycle(value: unknown, label: string): WorkerLifecycleState {
  if (
    typeof value !== "string" || !WORKER_LIFECYCLE_STATES.includes(value as WorkerLifecycleState)
  ) {
    throw new Error(`${label} is invalid`);
  }
  return value as WorkerLifecycleState;
}

const REMOVAL_STATES: readonly WorkerRemovalState[] = ["waiting", "held"];
const REMOVAL_PHASES: readonly WorkerRemovalPhase[] = [
  "awaiting_quiescence",
  "input_handoff",
  "awaiting_effects_end",
  "released",
  "completed",
];
const REMOVAL_CAUSES: readonly WorkerRemovalCause[] = [
  "recording_disabled",
  "source_unavailable",
  "ownership_unknown",
  "job_pending",
  "job_failed",
  "execution_unconfirmed",
  "storage_unavailable",
];

function removalEnum<T extends string>(value: unknown, values: readonly T[], label: string): T {
  if (typeof value !== "string" || !values.includes(value as T)) {
    throw new Error(`${label} is invalid`);
  }
  return value as T;
}

export function parseWorkerRemovalObservation(
  value: unknown,
  label = "Worker removal",
): WorkerRemovalObservation {
  const item = record(value, label);
  exact(item, ["operation_id", "state", "phase", "cause"], label);
  const operation_id = string(item.operation_id, `${label}.operation_id`);
  if (!operation_id || new TextEncoder().encode(operation_id).byteLength > 4096) {
    throw new Error(`${label}.operation_id is invalid`);
  }
  return {
    operation_id,
    state: removalEnum(item.state, REMOVAL_STATES, `${label}.state`),
    phase: removalEnum(item.phase, REMOVAL_PHASES, `${label}.phase`),
    cause: optional(
      item.cause,
      `${label}.cause`,
      (value, path) => removalEnum(value, REMOVAL_CAUSES, path),
    ),
  };
}

export function parseWorkerSummary(
  value: unknown,
  label = "Worker summary",
): ParsedWorkerSummary {
  const item = record(value, label);
  const availability = string(item.availability, `${label}.availability`);
  if (availability !== "observed" && availability !== "unavailable") {
    throw new Error(`${label}.availability is invalid`);
  }
  exact(
    item,
    [
      "runtime_id",
      "worker_id",
      "resource_key",
      "host_id",
      "display_name",
      "label",
      "profile",
      "singleton_key",
      "tags",
      "workspace",
      "availability",
      "state",
      "lifecycle",
      "worker_state",
      "restore_observation_token",
      "removal",
      "last_seen_at",
      "pinned",
      "retention_state",
      "implementation",
      "workdir_attachments",
      "diagnostics",
    ],
    label,
  );
  const workerState = optional(
    item.worker_state,
    `${label}.worker_state`,
    workerStateSnapshot,
  );
  if (availability === "unavailable" && workerState != null) {
    throw new Error(
      `${label}.worker_state must be absent when availability is unavailable`,
    );
  }
  return {
    runtime_id: string(item.runtime_id, `${label}.runtime_id`),
    worker_id: string(item.worker_id, `${label}.worker_id`),
    resource_key: string(item.resource_key, `${label}.resource_key`),
    host_id: string(item.host_id, `${label}.host_id`),
    display_name: item.display_name === undefined
      ? ""
      : string(item.display_name, `${label}.display_name`),
    label: string(item.label, `${label}.label`),
    profile: optional(item.profile, `${label}.profile`, string),
    singleton_key: optional(
      item.singleton_key,
      `${label}.singleton_key`,
      string,
    ),
    tags: item.tags === undefined ? [] : array(item.tags, `${label}.tags`, string),
    workspace: workspaceSummary(item.workspace, `${label}.workspace`),
    availability,
    state: string(item.state, `${label}.state`),
    lifecycle: optional(item.lifecycle, `${label}.lifecycle`, workerLifecycle),
    worker_state: workerState,
    removal: optional(item.removal, `${label}.removal`, parseWorkerRemovalObservation),
    restore_observation_token: optional(
      item.restore_observation_token,
      `${label}.restore_observation_token`,
      (value, path) => {
        const token = string(value, path);
        if (new TextEncoder().encode(token).byteLength > 256) {
          throw new Error(`${path} exceeds its 256 UTF-8 byte limit`);
        }
        return token;
      },
    ),
    last_seen_at: optional(item.last_seen_at, `${label}.last_seen_at`, string),
    pinned: item.pinned === undefined ? false : boolean(item.pinned, `${label}.pinned`),
    retention_state: item.retention_state === undefined
      ? ""
      : string(item.retention_state, `${label}.retention_state`),
    implementation: implementationSummary(
      item.implementation,
      `${label}.implementation`,
    ),
    workdir_attachments: item.workdir_attachments === undefined ? undefined : array(
      item.workdir_attachments,
      `${label}.workdir_attachments`,
      workerWorkdirAttachment,
    ),
    diagnostics: item.diagnostics === undefined
      ? []
      : array(item.diagnostics, `${label}.diagnostics`, diagnostic),
  };
}

export function parseWorkerListResponse(
  value: unknown,
): ParsedWorkerListResponse {
  const item = record(value, "Worker list response");
  exact(
    item,
    ["workspace_id", "limit", "items", "source", "diagnostics"],
    "Worker list response",
  );
  return {
    workspace_id: string(item.workspace_id, "workspace_id"),
    limit: unsignedInteger(item.limit, "limit"),
    items: array(item.items, "items", parseWorkerSummary),
    source: string(item.source, "source"),
    diagnostics: item.diagnostics === undefined
      ? []
      : array(item.diagnostics, "diagnostics", diagnostic),
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
      "display_name",
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
      "occupied_by",
    ],
    label,
  );
  const materializerKind = string(
    item.materializer_kind,
    `${label}.materializer_kind`,
  );
  if (materializerKind !== "runtime_git_clone") {
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
    display_name: optional(
      item.display_name,
      `${label}.display_name`,
      string,
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
    occupied_by: occupied,
  };
}

function runtimeWorkdirAttachment(
  value: unknown,
  label: string,
): RuntimeWorkerWorkdirAttachmentSummary {
  const item = record(value, label);
  exact(item, ["alias", "working_directory"], label);
  return {
    alias: string(item.alias, `${label}.alias`),
    working_directory: runtimeWorkingDirectory(
      item.working_directory,
      `${label}.working_directory`,
    ),
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
      "lifecycle",
      "last_seen_at",
      "pinned",
      "retention_state",
      "implementation",
      "workdir_attachments",
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
    lifecycle: optional(item.lifecycle, `${label}.lifecycle`, workerLifecycle),
    last_seen_at: nullableString(item.last_seen_at, `${label}.last_seen_at`),
    pinned: boolean(item.pinned, `${label}.pinned`),
    retention_state: string(item.retention_state, `${label}.retention_state`),
    implementation: implementationSummary(
      item.implementation,
      `${label}.implementation`,
    ),
    workdir_attachments: item.workdir_attachments === undefined ? undefined : array(
      item.workdir_attachments,
      `${label}.workdir_attachments`,
      runtimeWorkdirAttachment,
    ),
    diagnostics: array(item.diagnostics, `${label}.diagnostics`, diagnostic),
  };
}

export function parseWorkerLaunchWorkerSummary(
  value: unknown,
  label = "Worker launch summary",
): WorkerLaunchWorkerSummary {
  return workerSummary(value, label);
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
  exact(item, ["alias", "working_directory_id", "relative_cwd"], label);
  return {
    alias: string(item.alias, `${label}.alias`),
    working_directory_id: string(
      item.working_directory_id,
      `${label}.working_directory_id`,
    ),
    relative_cwd: nullableString(item.relative_cwd, `${label}.relative_cwd`),
  };
}

function featureConnectionsRequest(
  value: unknown,
  label: string,
): CreateWorkspaceWorkerRequest["feature_connections"] {
  const owners = new Set<string>();
  const encodedBytes = (input: unknown) => new TextEncoder().encode(JSON.stringify(input)).length;
  const connections = array(value, label, (input, itemLabel) => {
    const item = record(input, itemLabel);
    exact(item, ["feature_id", "version", "payload"], itemLabel);
    const feature_id = string(item.feature_id, `${itemLabel}.feature_id`);
    if (feature_id.length > MAX_FEATURE_INITIALIZATION_ID_BYTES ||
      !/^[A-Za-z0-9_./-]+:[A-Za-z0-9_:./-]+$/.test(feature_id)) {
      throw new Error(`${itemLabel}.feature_id must be a bounded source-qualified identifier`);
    }
    if (owners.has(feature_id)) throw new Error(`${label} has a duplicate Feature owner`);
    owners.add(feature_id);
    const version = unsignedInteger(item.version, `${itemLabel}.version`);
    if (version === 0 || version > 0xffff_ffff) {
      throw new Error(`${itemLabel}.version must be a nonzero compatibility epoch`);
    }
    const payload = record(item.payload, `${itemLabel}.payload`);
    if (encodedBytes(payload) > MAX_FEATURE_INITIALIZATION_PAYLOAD_BYTES) {
      throw new Error(`${itemLabel}.payload exceeds its byte limit`);
    }
    return { feature_id, version, payload };
  });
  if (connections.length > MAX_FEATURE_INITIALIZATIONS || encodedBytes(connections) > MAX_FEATURE_INITIALIZATION_BYTES) {
    throw new Error(`${label} exceeds its initialization limit`);
  }
  // Only the Backend's registered owner interprets the payload and authorizes it.
  connections.sort((left, right) => left.feature_id < right.feature_id ? -1 : left.feature_id > right.feature_id ? 1 : 0);
  // The checks above establish the generated ID-length, version-range and array-length
  // brands. Narrow only at this validated boundary, as with policy authorization keys.
  return connections as CreateWorkspaceWorkerRequest["feature_connections"];
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

function invocationValue(
  value: unknown,
  label: string,
): Extract<
  Extract<
    Segment,
    { kind: "feature_invoke" }
  >["invocation"]["arguments"][number],
  { name: string }
>["value"] {
  const item = record(value, label);
  const kind = string(item.kind, `${label}.kind`);
  exact(item, ["kind", "value"], label);
  if (kind === "string") {
    return { kind, value: string(item.value, `${label}.value`) };
  }
  if (kind === "integer") {
    const parsed = number(item.value, `${label}.value`);
    if (!Number.isSafeInteger(parsed)) {
      throw new Error(`${label}.value must be a safe integer`);
    }
    if (parsed < -2147483648 || parsed > 2147483647) {
      throw new Error(`${label}.value must be a signed 32-bit integer`);
    }
    return { kind, value: parsed };
  }
  if (kind === "boolean") {
    return { kind, value: boolean(item.value, `${label}.value`) };
  }
  throw new Error(`${label}.kind is invalid`);
}

function featureInvocation(
  value: unknown,
  label: string,
): Extract<Segment, { kind: "feature_invoke" }>["invocation"] {
  const item = record(value, label);
  exact(item, ["invocation_id", "identity", "name", "arguments"], label);
  return {
    invocation_id: string(item.invocation_id, `${label}.invocation_id`),
    identity: string(item.identity, `${label}.identity`),
    name: string(item.name, `${label}.name`),
    arguments: array(
      item.arguments,
      `${label}.arguments`,
      (argument, argumentLabel) => {
        const entry = record(argument, argumentLabel);
        exact(entry, ["name", "value"], argumentLabel);
        return {
          name: string(entry.name, `${argumentLabel}.name`),
          value: invocationValue(entry.value, `${argumentLabel}.value`),
        };
      },
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
    case "feature_invoke":
      exact(item, ["kind", "invocation"], label);
      return {
        kind,
        invocation: featureInvocation(item.invocation, `${label}.invocation`),
      };
    case "flow":
      exact(item, ["kind", "selector"], label);
      return { kind, selector: string(item.selector, `${label}.selector`) };
    case "unknown":
      throw new Error(`${label}.kind is not supported by Worker creation`);
  }
  const exhaustive: never = kind;
  throw new Error(`${label}.kind is invalid: ${exhaustive}`);
}

function creationAuthorization(value: unknown): WorkerCreationAuthorization {
  if (value === undefined) return { kind: "assignment", assignment: null };
  const item = record(value, "authorization");
  if (item.kind === "assignment") {
    exact(item, ["kind", "assignment"], "authorization");
    return {
      kind: "assignment",
      ...(item.assignment === undefined ? {} : {
        assignment: item.assignment === null ? null : parseAssignment(item.assignment),
      }),
    };
  }
  if (item.kind === "policy") {
    exact(item, ["kind", "policy_id", "policy_authorization_key"], "authorization");
    const policyId = string(item.policy_id, "policy_id");
    if (!/^[a-z][a-z0-9_-]{0,95}$/.test(policyId)) throw new Error("invalid policy_id");
    const authorizationKey = string(
      item.policy_authorization_key,
      "policy_authorization_key",
    );
    if (
      authorizationKey.length === 0 ||
      new TextEncoder().encode(authorizationKey).byteLength > 512 ||
      /\p{Cc}/u.test(authorizationKey)
    ) {
      throw new Error(
        "policy_authorization_key must contain 1..512 UTF-8 bytes and no control characters",
      );
    }
    return {
      kind: "policy",
      policy_id: policyId,
      policy_authorization_key: authorizationKey as Extract<
        WorkerCreationAuthorization,
        { kind: "policy" }
      >["policy_authorization_key"],
    };
  }
  throw new Error("authorization.kind must be assignment or policy");
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
      "initial_submit",
      "workdir_attachments",
      "feature_connections",
      "control_operation_id",
      "authorization",
    ],
    "Worker create request",
  );
  return {
    runtime_id: string(item.runtime_id, "runtime_id"),
    display_name: string(item.display_name, "display_name"),
    profile: nullableString(item.profile, "profile"),
    initial_submit: array(item.initial_submit, "initial_submit", segment),
    workdir_attachments: array(
      item.workdir_attachments,
      "workdir_attachments",
      workingDirectorySelection,
    ),
    feature_connections: featureConnectionsRequest(
      item.feature_connections,
      "feature_connections",
    ),
    authorization: creationAuthorization(item.authorization),
    control_operation_id: nullableString(
      item.control_operation_id,
      "control_operation_id",
    ),
  };
}
