import type {
  CompanionLifecycleState,
  CompanionMessageDisposition,
  CompanionMessageResponse,
  CompanionStatusResponse,
  CompanionTranscriptItem,
  CompanionTranscriptProjection,
  Diagnostic,
  DiagnosticSeverity,
  WorkspaceWorkerDiscoveryItem,
  WorkspaceWorkerSubject,
} from "$lib/generated/companion-api";

const MAX_TRANSCRIPT_ITEMS = 200;
const MAX_DIAGNOSTICS = 100;
const MAX_CONTENT_LENGTH = 64 * 1024;

export function parseCompanionStatusResponse(
  value: unknown,
): CompanionStatusResponse {
  const record = strictRecord(value, [
    "state",
    "worker",
    "transport",
    "diagnostics",
  ]);
  const transport = strictRecord(record.transport, ["mode", "available"]);
  const diagnostics = boundedArray(record.diagnostics, MAX_DIAGNOSTICS).map(
    parseDiagnostic,
  );

  return {
    state: lifecycleState(record.state),
    worker: record.worker === null ? null : parseWorker(record.worker),
    transport: {
      mode: boundedString(transport.mode, 100),
      available: booleanValue(transport.available),
    },
    diagnostics,
  };
}

export function parseCompanionMessageResponse(
  value: unknown,
): CompanionMessageResponse {
  const record = strictRecord(value, ["state", "message"]);
  return {
    state: messageDisposition(record.state),
    message: boundedString(record.message, 8 * 1024),
  };
}

export function parseCompanionTranscriptProjection(
  value: unknown,
): CompanionTranscriptProjection {
  const record = strictRecord(value, [
    "state",
    "start",
    "limit",
    "total",
    "next",
    "items",
  ]);
  const start = boundedInteger(record.start);
  const limit = boundedInteger(record.limit);
  if (limit > MAX_TRANSCRIPT_ITEMS) {
    throw new TypeError("Companion transcript limit is out of range");
  }
  const items = boundedArray(record.items, limit).map(parseTranscriptItem);
  const total = boundedInteger(record.total);
  if (total < items.length) {
    throw new TypeError("Companion transcript total is smaller than its items");
  }
  const next = record.next === null ? null : boundedInteger(record.next);

  return {
    state: lifecycleState(record.state),
    start,
    limit,
    total,
    next,
    items,
  };
}

function parseTranscriptItem(value: unknown): CompanionTranscriptItem {
  const record = strictRecord(value, [
    "sequence",
    "role",
    "content",
    "created_at",
  ]);
  const role = record.role;
  if (role !== "user" && role !== "assistant") {
    throw new TypeError("Companion transcript role is not user-visible");
  }
  return {
    sequence: boundedInteger(record.sequence),
    role,
    content: boundedString(record.content, MAX_CONTENT_LENGTH),
    created_at: boundedString(record.created_at, 100),
  };
}

function parseWorker(value: unknown): WorkspaceWorkerDiscoveryItem {
  const record = strictRecord(value, [
    "subject",
    "resource_key",
    "display_name",
    "profile",
    "status",
  ], ["status"]);
  const subject = parseWorkerSubject(record.subject);
  const resourceKey = boundedString(record.resource_key, 100);
  if (!/^W-[1-9][0-9]*$/.test(resourceKey)) {
    throw new TypeError("Companion worker resource_key is not canonical");
  }
  return {
    subject,
    resource_key: resourceKey,
    display_name: boundedString(record.display_name, 256),
    profile: nullableString(record.profile, 256),
    ...(record.status === undefined
      ? {}
      : { status: nullableString(record.status, 100) }),
  };
}

function parseWorkerSubject(value: unknown): WorkspaceWorkerSubject {
  const record = strictRecord(value, ["kind", "runtime_id", "worker_id"]);
  if (record.kind !== "runtime_worker") {
    throw new TypeError("Companion worker subject kind is invalid");
  }
  return {
    kind: "runtime_worker",
    runtime_id: boundedString(record.runtime_id, 256),
    worker_id: boundedString(record.worker_id, 256),
  };
}

function parseDiagnostic(value: unknown): Diagnostic {
  const record = strictRecord(value, ["code", "severity", "message"]);
  return {
    code: boundedString(record.code, 256),
    severity: diagnosticSeverity(record.severity),
    message: boundedString(record.message, 4 * 1024),
  };
}

function lifecycleState(value: unknown): CompanionLifecycleState {
  if (value !== "idle" && value !== "running" && value !== "stopped") {
    throw new TypeError("Companion lifecycle state is invalid");
  }
  return value;
}

function messageDisposition(value: unknown): CompanionMessageDisposition {
  if (value !== "accepted" && value !== "rejected") {
    throw new TypeError("Companion message disposition is invalid");
  }
  return value;
}

function diagnosticSeverity(value: unknown): DiagnosticSeverity {
  if (value !== "info" && value !== "warning" && value !== "error") {
    throw new TypeError("Companion diagnostic severity is invalid");
  }
  return value;
}

function strictRecord(
  value: unknown,
  keys: readonly string[],
  optionalKeys: readonly string[] = [],
): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new TypeError("Companion API value is not an object");
  }
  const record = value as Record<string, unknown>;
  const allowed = new Set(keys);
  for (const key of Object.keys(record)) {
    if (!allowed.has(key)) {
      throw new TypeError(`Companion API field is not public: ${key}`);
    }
  }
  const optional = new Set(optionalKeys);
  for (const key of keys) {
    if (!optional.has(key) && !(key in record)) {
      throw new TypeError(`Companion API field is missing: ${key}`);
    }
  }
  return record;
}

function boundedArray(value: unknown, limit: number): unknown[] {
  if (!Array.isArray(value) || value.length > limit) {
    throw new TypeError("Companion API array is invalid or exceeds its limit");
  }
  return value;
}

function boundedString(value: unknown, limit: number): string {
  if (typeof value !== "string" || value.length > limit) {
    throw new TypeError("Companion API string is invalid or exceeds its limit");
  }
  return value;
}

function nullableString(value: unknown, limit: number): string | null {
  return value === null ? null : boundedString(value, limit);
}

function booleanValue(value: unknown): boolean {
  if (typeof value !== "boolean") {
    throw new TypeError("Companion API value is not a boolean");
  }
  return value;
}

function boundedInteger(value: unknown): number {
  if (!Number.isSafeInteger(value) || (value as number) < 0) {
    throw new TypeError("Companion API value is not a non-negative integer");
  }
  return value as number;
}
