import type {
  Diagnostic,
  WorkingDirectoryCleanupTarget,
  WorkingDirectoryCreateRequest,
  WorkingDirectoryCreateResponse,
  WorkingDirectoryDetailResponse,
  WorkingDirectoryListResponse,
  WorkingDirectoryOccupancy,
  WorkingDirectorySummary,
} from "../../generated/workdir-api";

const SUMMARY_KEYS = new Set([
  "working_directory_id",
  "repository_key",
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
]);
const CREATE_REQUEST_KEYS = new Set([
  "runtime_id",
  "repository_key",
  "selector",
  "operation_id",
]);
const DIAGNOSTIC_KEYS = new Set(["code", "severity", "message"]);
const CLEANUP_TARGET_KEYS = new Set([
  "kind",
  "working_directory_id",
  "repository_key",
]);
const OCCUPANCY_KEYS = new Set([
  "runtime_id",
  "worker_id",
  "display_name",
  "linked_at",
]);

export function parseWorkingDirectoryListResponse(
  value: unknown,
): WorkingDirectoryListResponse {
  const record = exactRecord(
    value,
    new Set(["workspace_id", "items", "diagnostics"]),
    "Workdir list response",
  );
  return {
    workspace_id: stringField(record, "workspace_id"),
    items: arrayField(record, "items").map(parseWorkingDirectorySummary),
    diagnostics: arrayField(record, "diagnostics").map(parseDiagnostic),
  };
}

export function parseWorkingDirectoryDetailResponse(
  value: unknown,
): WorkingDirectoryDetailResponse {
  return parseDetailLike(value, "Workdir detail response");
}

export function parseWorkingDirectoryCreateResponse(
  value: unknown,
): WorkingDirectoryCreateResponse {
  return parseDetailLike(value, "Workdir create response");
}

export function validateWorkingDirectoryCreateRequest(
  value: unknown,
): WorkingDirectoryCreateRequest {
  const record = exactRecord(
    value,
    CREATE_REQUEST_KEYS,
    "Workdir create request",
  );
  const request: WorkingDirectoryCreateRequest = {
    repository_key: stringField(record, "repository_key"),
  };
  assignOptionalString(request, record, "runtime_id");
  assignOptionalString(request, record, "selector");
  assignOptionalString(request, record, "operation_id");
  return request;
}

function parseDetailLike(
  value: unknown,
  label: string,
): WorkingDirectoryDetailResponse {
  const record = exactRecord(
    value,
    new Set(["workspace_id", "runtime_id", "item", "diagnostics"]),
    label,
  );
  return {
    workspace_id: stringField(record, "workspace_id"),
    runtime_id: stringField(record, "runtime_id"),
    item: parseWorkingDirectorySummary(record.item),
    diagnostics: arrayField(record, "diagnostics").map(parseDiagnostic),
  };
}

export function parseWorkingDirectorySummary(
  value: unknown,
): WorkingDirectorySummary {
  const record = exactRecord(value, SUMMARY_KEYS, "Workdir summary");
  const summary: WorkingDirectorySummary = {
    working_directory_id: stringField(record, "working_directory_id"),
    repository_key: stringField(record, "repository_key"),
    materializer_kind: enumField(record, "materializer_kind", [
      "runtime_git_cache",
      "local_git_worktree",
    ]),
    status: enumField(record, "status", [
      "active",
      "cleanup_pending",
      "corrupted",
      "not_found",
      "unknown",
    ]),
  };
  assignOptionalString(summary, record, "creation_selector");
  assignOptionalString(summary, record, "creation_ref");
  assignOptionalString(summary, record, "creation_tree");
  assignOptionalString(summary, record, "current_selector");
  assignOptionalString(summary, record, "current_ref");
  assignOptionalString(summary, record, "current_tree");
  assignOptionalString(summary, record, "cleanliness");
  assignOptionalString(summary, record, "primary_worker_id");
  if (record.observed_at_epoch_seconds !== undefined) {
    const observedAt = record.observed_at_epoch_seconds;
    if (observedAt === null) {
      summary.observed_at_epoch_seconds = null;
    } else {
      if (!Number.isSafeInteger(observedAt) || Number(observedAt) < 0) {
        throw new Error(
          "Workdir summary.observed_at_epoch_seconds must be a non-negative safe integer or null",
        );
      }
      summary.observed_at_epoch_seconds = Number(observedAt);
    }
  }
  if (record.cleanup_target !== undefined) {
    summary.cleanup_target = record.cleanup_target === null
      ? null
      : parseCleanupTarget(record.cleanup_target);
  }
  if (record.occupied_by !== undefined) {
    summary.occupied_by = record.occupied_by === null
      ? null
      : parseOccupancy(record.occupied_by);
  }
  return summary;
}

function parseCleanupTarget(value: unknown): WorkingDirectoryCleanupTarget {
  const record = exactRecord(
    value,
    CLEANUP_TARGET_KEYS,
    "Workdir cleanup target",
  );
  return {
    kind: stringField(record, "kind"),
    working_directory_id: stringField(record, "working_directory_id"),
    repository_key: stringField(record, "repository_key"),
  };
}

function parseOccupancy(value: unknown): WorkingDirectoryOccupancy {
  const record = exactRecord(value, OCCUPANCY_KEYS, "Workdir occupancy");
  return {
    runtime_id: stringField(record, "runtime_id"),
    worker_id: stringField(record, "worker_id"),
    display_name: stringField(record, "display_name"),
    linked_at: stringField(record, "linked_at"),
  };
}

function parseDiagnostic(value: unknown): Diagnostic {
  const record = exactRecord(value, DIAGNOSTIC_KEYS, "Workdir diagnostic");
  return {
    code: stringField(record, "code"),
    severity: enumField(record, "severity", ["info", "warning", "error"]),
    message: stringField(record, "message"),
  };
}

function exactRecord(
  value: unknown,
  keys: ReadonlySet<string>,
  label: string,
): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
  const record = value as Record<string, unknown>;
  for (const key of Object.keys(record)) {
    if (!keys.has(key)) {
      throw new Error(`${label} contains unknown field ${key}`);
    }
  }
  return record;
}

function stringField(record: Record<string, unknown>, key: string): string {
  const value = record[key];
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`${key} must be a non-empty string`);
  }
  return value;
}

function arrayField(record: Record<string, unknown>, key: string): unknown[] {
  const value = record[key];
  if (!Array.isArray(value)) throw new Error(`${key} must be an array`);
  return value;
}

function enumField<T extends string>(
  record: Record<string, unknown>,
  key: string,
  values: readonly T[],
): T {
  const value = record[key];
  if (typeof value !== "string" || !values.includes(value as T)) {
    throw new Error(`${key} has an unsupported value`);
  }
  return value as T;
}

function assignOptionalString<T extends object>(
  target: T,
  source: Record<string, unknown>,
  key: string,
): void {
  const value = source[key];
  if (value === undefined) return;
  if (value === null) {
    (target as Record<string, unknown>)[key] = null;
    return;
  }
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`${key} must be a non-empty string or null`);
  }
  (target as Record<string, unknown>)[key] = value;
}
