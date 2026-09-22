import type {
  CleanupTargetKind,
  CleanupWorkdirCandidate,
  CleanupWorkdirCleanliness,
  CleanupWorkdirFileStatus,
  CleanupWorkerCandidate,
  Diagnostic,
  RuntimeCleanupExecutionResponse,
  RuntimeCleanupExecutionResult,
  RuntimeCleanupPlanResponse,
  RuntimeWorkerLifecycleResult,
  WorkerOperationState,
  WorkerRetentionResponse,
} from "$lib/generated/runtime-api";

const MAX_STRING_BYTES = 4_096;
const MAX_CANDIDATES = 1_000;
const MAX_RESULTS = 2_000;
const MAX_DIAGNOSTICS = 64;
const encoder = new TextEncoder();
type JsonObject = Record<string, unknown>;

const TARGET_KINDS = new Set<CleanupTargetKind>([
  "worker_delete",
  "workdir_clean_cleanup",
  "workdir_dirty_discard",
  "workdir_record_delete",
]);
const FILE_STATUSES = new Set<CleanupWorkdirFileStatus>([
  "pending",
  "present",
  "active",
  "cleanup_pending",
  "not_found",
  "corrupted",
  "failed",
  "unknown",
]);
const CLEANLINESS = new Set<CleanupWorkdirCleanliness>([
  "clean",
  "dirty",
  "unknown",
]);

export class RuntimeWorkerValidationError extends Error {
  constructor(message: string) {
    super(message.slice(0, 256));
    this.name = "RuntimeWorkerValidationError";
  }
}

function fail(path: string, message: string): never {
  throw new RuntimeWorkerValidationError(`${path} ${message}`);
}

function object(value: unknown, path: string): JsonObject {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return fail(path, "must be an object");
  }
  return value as JsonObject;
}

function exactKeys(
  value: JsonObject,
  required: readonly string[],
  optional: readonly string[],
  path: string,
): void {
  const allowed = new Set([...required, ...optional]);
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) fail(`${path}.${key}`, "is not part of the wire contract");
  }
  for (const key of required) {
    if (!Object.hasOwn(value, key)) fail(`${path}.${key}`, "is required");
  }
}

function boundedString(value: unknown, path: string, allowEmpty = false): string {
  if (typeof value !== "string") return fail(path, "must be a string");
  if (!allowEmpty && value.length === 0) return fail(path, "must not be empty");
  if (encoder.encode(value).byteLength > MAX_STRING_BYTES) {
    return fail(path, `must be at most ${MAX_STRING_BYTES} UTF-8 bytes`);
  }
  return value;
}

function boolean(value: unknown, path: string): boolean {
  if (typeof value !== "boolean") return fail(path, "must be a boolean");
  return value;
}

function array(value: unknown, path: string, max: number): unknown[] {
  if (!Array.isArray(value)) return fail(path, "must be an array");
  if (value.length > max) return fail(path, `must contain at most ${max} items`);
  return value;
}

function strings(value: unknown, path: string): string[] {
  return array(value, path, MAX_CANDIDATES).map((entry, index) =>
    boundedString(entry, `${path}[${index}]`)
  );
}

function optionalNullableString(value: unknown, path: string): string | null | undefined {
  if (value === undefined || value === null) return value;
  return boundedString(value, path);
}

function optionalNullableSafeInteger(value: unknown, path: string): number | null | undefined {
  if (value === undefined || value === null) return value;
  if (!Number.isSafeInteger(value) || (value as number) < 0) {
    return fail(path, "must be a non-negative safe integer");
  }
  return value as number;
}

function targetKind(value: unknown, path: string): CleanupTargetKind {
  if (typeof value !== "string" || !TARGET_KINDS.has(value as CleanupTargetKind)) {
    return fail(path, "has an unknown cleanup action");
  }
  return value as CleanupTargetKind;
}

function diagnostic(value: unknown, path: string): Diagnostic {
  const item = object(value, path);
  exactKeys(item, ["code", "severity", "message"], [], path);
  const severity = boundedString(item.severity, `${path}.severity`);
  if (severity !== "info" && severity !== "warning" && severity !== "error") {
    fail(`${path}.severity`, "has an unknown value");
  }
  return {
    code: boundedString(item.code, `${path}.code`),
    severity,
    message: boundedString(item.message, `${path}.message`, true),
  };
}

function diagnostics(value: unknown, path: string): Diagnostic[] {
  return array(value, path, MAX_DIAGNOSTICS).map((entry, index) =>
    diagnostic(entry, `${path}[${index}]`)
  );
}

function workerCandidate(value: unknown, path: string): CleanupWorkerCandidate {
  const item = object(value, path);
  exactKeys(
    item,
    [
      "target_id",
      "action",
      "worker_id",
      "runtime_worker_id",
      "runtime_id",
      "reason",
      "pinned",
      "retention_state",
      "linked_workdir_ids",
      "running_linked",
    ],
    ["blocking_reason", "estimated_reclaim_bytes"],
    path,
  );
  return {
    target_id: boundedString(item.target_id, `${path}.target_id`),
    action: targetKind(item.action, `${path}.action`),
    worker_id: boundedString(item.worker_id, `${path}.worker_id`),
    runtime_worker_id: boundedString(item.runtime_worker_id, `${path}.runtime_worker_id`),
    runtime_id: boundedString(item.runtime_id, `${path}.runtime_id`),
    reason: boundedString(item.reason, `${path}.reason`, true),
    blocking_reason: optionalNullableString(item.blocking_reason, `${path}.blocking_reason`),
    pinned: boolean(item.pinned, `${path}.pinned`),
    retention_state: boundedString(item.retention_state, `${path}.retention_state`),
    linked_workdir_ids: strings(item.linked_workdir_ids, `${path}.linked_workdir_ids`),
    running_linked: boolean(item.running_linked, `${path}.running_linked`),
    estimated_reclaim_bytes: optionalNullableSafeInteger(
      item.estimated_reclaim_bytes,
      `${path}.estimated_reclaim_bytes`,
    ),
  };
}

function workdirCandidate(value: unknown, path: string): CleanupWorkdirCandidate {
  const item = object(value, path);
  exactKeys(
    item,
    [
      "target_id",
      "action",
      "workdir_id",
      "runtime_id",
      "repository_key",
      "reason",
      "linked_worker_ids",
      "linked_running_worker_ids",
      "running_linked",
      "pinned_linked",
      "file_status",
      "cleanliness",
    ],
    ["blocking_reason", "estimated_reclaim_bytes"],
    path,
  );
  const fileStatus = boundedString(item.file_status, `${path}.file_status`);
  const cleanliness = boundedString(item.cleanliness, `${path}.cleanliness`);
  if (!FILE_STATUSES.has(fileStatus as CleanupWorkdirFileStatus)) {
    fail(`${path}.file_status`, "has an unknown value");
  }
  if (!CLEANLINESS.has(cleanliness as CleanupWorkdirCleanliness)) {
    fail(`${path}.cleanliness`, "has an unknown value");
  }
  return {
    target_id: boundedString(item.target_id, `${path}.target_id`),
    action: targetKind(item.action, `${path}.action`),
    workdir_id: boundedString(item.workdir_id, `${path}.workdir_id`),
    runtime_id: boundedString(item.runtime_id, `${path}.runtime_id`),
    repository_key: boundedString(item.repository_key, `${path}.repository_key`),
    reason: boundedString(item.reason, `${path}.reason`, true),
    blocking_reason: optionalNullableString(item.blocking_reason, `${path}.blocking_reason`),
    linked_worker_ids: strings(item.linked_worker_ids, `${path}.linked_worker_ids`),
    linked_running_worker_ids: strings(
      item.linked_running_worker_ids,
      `${path}.linked_running_worker_ids`,
    ),
    running_linked: boolean(item.running_linked, `${path}.running_linked`),
    pinned_linked: boolean(item.pinned_linked, `${path}.pinned_linked`),
    file_status: fileStatus as CleanupWorkdirFileStatus,
    cleanliness: cleanliness as CleanupWorkdirCleanliness,
    estimated_reclaim_bytes: optionalNullableSafeInteger(
      item.estimated_reclaim_bytes,
      `${path}.estimated_reclaim_bytes`,
    ),
  };
}

export function parseWorkerRetentionResponse(value: unknown): WorkerRetentionResponse {
  const item = object(value, "Worker retention response");
  exactKeys(
    item,
    ["workspace_id", "runtime_id", "worker_id", "pinned", "retention_state"],
    [],
    "Worker retention response",
  );
  return {
    workspace_id: boundedString(item.workspace_id, "Worker retention response.workspace_id"),
    runtime_id: boundedString(item.runtime_id, "Worker retention response.runtime_id"),
    worker_id: boundedString(item.worker_id, "Worker retention response.worker_id"),
    pinned: boolean(item.pinned, "Worker retention response.pinned"),
    retention_state: boundedString(
      item.retention_state,
      "Worker retention response.retention_state",
    ),
  };
}

const WORKER_OPERATION_STATES = new Set<WorkerOperationState>([
  "accepted",
  "unsupported",
  "rejected",
]);

export function parseRuntimeWorkerLifecycleResult(
  value: unknown,
): RuntimeWorkerLifecycleResult {
  const item = object(value, "Runtime Worker lifecycle result");
  exactKeys(
    item,
    ["state", "runtime_id", "worker_id", "diagnostics"],
    [],
    "Runtime Worker lifecycle result",
  );
  const state = boundedString(item.state, "Runtime Worker lifecycle result.state");
  if (!WORKER_OPERATION_STATES.has(state as WorkerOperationState)) {
    fail("Runtime Worker lifecycle result.state", "has an unknown value");
  }
  return {
    state: state as WorkerOperationState,
    runtime_id: boundedString(
      item.runtime_id,
      "Runtime Worker lifecycle result.runtime_id",
    ),
    worker_id: boundedString(
      item.worker_id,
      "Runtime Worker lifecycle result.worker_id",
    ),
    diagnostics: diagnostics(
      item.diagnostics,
      "Runtime Worker lifecycle result.diagnostics",
    ),
  };
}

export function parseRuntimeCleanupPlan(value: unknown): RuntimeCleanupPlanResponse {
  const item = object(value, "Runtime cleanup plan");
  exactKeys(
    item,
    [
      "workspace_id",
      "runtime_id",
      "generated_at",
      "revision",
      "digest",
      "workers",
      "workdirs",
      "diagnostics",
    ],
    [],
    "Runtime cleanup plan",
  );
  return {
    workspace_id: boundedString(item.workspace_id, "Runtime cleanup plan.workspace_id"),
    runtime_id: boundedString(item.runtime_id, "Runtime cleanup plan.runtime_id"),
    generated_at: boundedString(item.generated_at, "Runtime cleanup plan.generated_at"),
    revision: boundedString(item.revision, "Runtime cleanup plan.revision"),
    digest: boundedString(item.digest, "Runtime cleanup plan.digest"),
    workers: array(item.workers, "Runtime cleanup plan.workers", MAX_CANDIDATES).map(
      (entry, index) => workerCandidate(entry, `Runtime cleanup plan.workers[${index}]`),
    ),
    workdirs: array(item.workdirs, "Runtime cleanup plan.workdirs", MAX_CANDIDATES).map(
      (entry, index) => workdirCandidate(entry, `Runtime cleanup plan.workdirs[${index}]`),
    ),
    diagnostics: diagnostics(item.diagnostics, "Runtime cleanup plan.diagnostics"),
  };
}

function executionResult(value: unknown, path: string): RuntimeCleanupExecutionResult {
  const item = object(value, path);
  exactKeys(item, ["target_id", "action", "status", "message"], [], path);
  return {
    target_id: boundedString(item.target_id, `${path}.target_id`),
    action: targetKind(item.action, `${path}.action`),
    status: boundedString(item.status, `${path}.status`),
    message: boundedString(item.message, `${path}.message`, true),
  };
}

export function parseRuntimeCleanupExecution(
  value: unknown,
): RuntimeCleanupExecutionResponse {
  const item = object(value, "Runtime cleanup execution");
  exactKeys(
    item,
    ["workspace_id", "runtime_id", "executed_at", "results", "plan_after", "diagnostics"],
    [],
    "Runtime cleanup execution",
  );
  return {
    workspace_id: boundedString(item.workspace_id, "Runtime cleanup execution.workspace_id"),
    runtime_id: boundedString(item.runtime_id, "Runtime cleanup execution.runtime_id"),
    executed_at: boundedString(item.executed_at, "Runtime cleanup execution.executed_at"),
    results: array(item.results, "Runtime cleanup execution.results", MAX_RESULTS).map(
      (entry, index) => executionResult(entry, `Runtime cleanup execution.results[${index}]`),
    ),
    plan_after: parseRuntimeCleanupPlan(item.plan_after),
    diagnostics: diagnostics(item.diagnostics, "Runtime cleanup execution.diagnostics"),
  };
}
