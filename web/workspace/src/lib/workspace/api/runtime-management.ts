import type {
  Diagnostic,
  PutRuntimeTrustKeyRequest,
  RevokeRuntimeTrustKeyRequest,
  RuntimeIdentityAuthority,
  RuntimeManagementSummary,
  RuntimeSourceKind,
  RuntimeSourceStatus,
  RuntimeSourceSummary,
  RuntimeTrustAuditAction,
  RuntimeTrustAuditEntry,
  RuntimeTrustConflictKind,
  RuntimeTrustConflictResponse,
  RuntimeTrustKeyState,
  RuntimeTrustKeyStatus,
  WorkspaceRuntimeDetail,
  WorkspaceRuntimeResource,
} from "$lib/generated/workspace-api.ts";
import type { ListResponse } from "$lib/workspace/sidebar/types";
import { workspaceApiPath } from "./http.ts";

export type WorkspaceRuntimeList = ListResponse<WorkspaceRuntimeResource>;

const LIMITS = {
  runtimeItems: 200,
  auditEntries: 20,
  hostIds: 128,
  diagnostics: 64,
  idBytes: 256,
  labelBytes: 512,
  kindBytes: 128,
  statusBytes: 128,
  noteBytes: 2_048,
  endpointBytes: 4_096,
  publicKeyBytes: 16 * 1_024,
  fingerprintBytes: 512,
  timestampBytes: 128,
  diagnosticCodeBytes: 128,
  diagnosticMessageBytes: 2_048,
  conflictMessageBytes: 1_024,
  responseBytes: 512 * 1_024,
} as const;

const SOURCE_KINDS = new Set<RuntimeSourceKind>([
  "embedded_worker_runtime",
  "remote_http",
]);
const SOURCE_STATUSES = new Set<RuntimeSourceStatus>(["active", "reserved"]);
const IDENTITY_AUTHORITIES = new Set<RuntimeIdentityAuthority>([
  "runtime_registry_projection",
  "server_runtime_configuration",
]);
const DIAGNOSTIC_SEVERITIES = new Set(["info", "warning", "error"]);
const TRUST_STATUSES = new Set<RuntimeTrustKeyStatus>([
  "unconfigured",
  "active",
  "revoked",
]);
const AUDIT_ACTIONS = new Set<RuntimeTrustAuditAction>([
  "created",
  "replaced",
  "reactivated",
  "revoked",
]);
const CONFLICT_KINDS = new Set<RuntimeTrustConflictKind>([
  "stale_revision",
  "fingerprint_in_use",
]);

const encoder = new TextEncoder();
type JsonObject = Record<string, unknown>;

export class RuntimeManagementValidationError extends Error {
  constructor(message: string) {
    super(message.slice(0, 256));
    this.name = "RuntimeManagementValidationError";
  }
}

export class RuntimeTrustConflictError extends Error {
  readonly conflict: RuntimeTrustConflictResponse;

  constructor(conflict: RuntimeTrustConflictResponse) {
    super(conflict.message);
    this.name = "RuntimeTrustConflictError";
    this.conflict = conflict;
  }
}

export class RuntimeTrustRequestError extends Error {
  readonly field: "public_key" | null;

  constructor(message: string, field: "public_key" | null = null) {
    super(message.slice(0, 256));
    this.name = "RuntimeTrustRequestError";
    this.field = field;
  }
}

function fail(path: string, message: string): never {
  throw new RuntimeManagementValidationError(`${path} ${message}`);
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
    if (!allowed.has(key)) {
      fail(`${path}.${key}`, "is not part of the wire contract");
    }
  }
  for (const key of required) {
    if (!Object.hasOwn(value, key)) fail(`${path}.${key}`, "is required");
  }
}

function array(value: unknown, path: string, max: number): unknown[] {
  if (!Array.isArray(value)) return fail(path, "must be an array");
  if (value.length > max) {
    return fail(path, `must contain at most ${max} items`);
  }
  return value;
}

function boundedString(
  value: unknown,
  path: string,
  maxBytes: number,
  allowEmpty = false,
): string {
  if (typeof value !== "string") return fail(path, "must be a string");
  if (!allowEmpty && value.length === 0) return fail(path, "must not be empty");
  if (encoder.encode(value).byteLength > maxBytes) {
    return fail(path, `must be at most ${maxBytes} UTF-8 bytes`);
  }
  return value;
}

function boolean(value: unknown, path: string): boolean {
  if (typeof value !== "boolean") return fail(path, "must be a boolean");
  return value;
}

function safeInteger(value: unknown, path: string, minimum = 0): number {
  if (
    typeof value !== "number" || !Number.isSafeInteger(value) || value < minimum
  ) {
    return fail(path, `must be a safe integer of at least ${minimum}`);
  }
  return value;
}

function safeRevision(value: unknown, path: string): number {
  return safeInteger(value, path, 1);
}

function optionalNullableString(
  value: unknown,
  path: string,
  maxBytes: number,
  allowEmpty = false,
): string | null | undefined {
  if (value === undefined || value === null) return value;
  return boundedString(value, path, maxBytes, allowEmpty);
}

function optionalRevision(
  value: unknown,
  path: string,
): number | undefined {
  if (value === undefined || value === null) return undefined;
  return safeRevision(value, path);
}

function optionalNullableRevision(
  value: unknown,
  path: string,
): number | null | undefined {
  if (value === undefined || value === null) return value;
  return safeRevision(value, path);
}

function timestamp(value: unknown, path: string): string {
  const result = boundedString(value, path, LIMITS.timestampBytes);
  if (
    !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/
      .test(result)
  ) {
    return fail(path, "must be an RFC 3339 timestamp");
  }
  return result;
}

function optionalNullableTimestamp(
  value: unknown,
  path: string,
): string | null | undefined {
  if (value === undefined || value === null) return value;
  return timestamp(value, path);
}

function enumValue<T extends string>(
  value: unknown,
  path: string,
  variants: ReadonlySet<T>,
): T {
  const result = boundedString(value, path, LIMITS.kindBytes);
  if (!variants.has(result as T)) {
    return fail(path, "contains an unknown enum value");
  }
  return result as T;
}

function diagnostic(value: unknown, path: string): Diagnostic {
  const item = object(value, path);
  exactKeys(item, ["code", "severity", "message"], [], path);
  const severity = enumValue(
    item.severity,
    `${path}.severity`,
    DIAGNOSTIC_SEVERITIES,
  ) as Diagnostic["severity"];
  return {
    code: boundedString(item.code, `${path}.code`, LIMITS.diagnosticCodeBytes),
    severity,
    message: boundedString(
      item.message,
      `${path}.message`,
      LIMITS.diagnosticMessageBytes,
      true,
    ),
  };
}

function runtimeSource(value: unknown, path: string): RuntimeSourceSummary {
  const item = object(value, path);
  exactKeys(item, ["kind", "status", "identity_authority", "note"], [], path);
  return {
    kind: enumValue(item.kind, `${path}.kind`, SOURCE_KINDS),
    status: enumValue(item.status, `${path}.status`, SOURCE_STATUSES),
    identity_authority: enumValue(
      item.identity_authority,
      `${path}.identity_authority`,
      IDENTITY_AUTHORITIES,
    ),
    note: boundedString(item.note, `${path}.note`, LIMITS.noteBytes, true),
  };
}

function runtimeManagement(
  value: unknown,
  path: string,
): RuntimeManagementSummary {
  const item = object(value, path);
  exactKeys(
    item,
    [
      "built_in",
      "config_managed",
      "removable",
      "endpoint_configured",
      "token_ref_configured",
    ],
    [],
    path,
  );
  return {
    built_in: boolean(item.built_in, `${path}.built_in`),
    config_managed: boolean(item.config_managed, `${path}.config_managed`),
    removable: boolean(item.removable, `${path}.removable`),
    endpoint_configured: boolean(
      item.endpoint_configured,
      `${path}.endpoint_configured`,
    ),
    token_ref_configured: boolean(
      item.token_ref_configured,
      `${path}.token_ref_configured`,
    ),
  };
}

function runtimeResource(
  value: unknown,
  path: string,
): WorkspaceRuntimeResource {
  const item = object(value, path);
  exactKeys(
    item,
    [
      "management",
      "runtime_id",
      "label",
      "kind",
      "status",
      "source",
      "host_ids",
      "worker_creation_available",
      "os",
      "arch",
      "diagnostics",
    ],
    [],
    path,
  );
  const hostIds = array(item.host_ids, `${path}.host_ids`, LIMITS.hostIds).map(
    (entry, index) =>
      boundedString(
        entry,
        `${path}.host_ids[${index}]`,
        LIMITS.idBytes,
      ),
  );
  if (new Set(hostIds).size !== hostIds.length) {
    fail(`${path}.host_ids`, "must not contain duplicate IDs");
  }
  return {
    management: runtimeManagement(item.management, `${path}.management`),
    runtime_id: boundedString(
      item.runtime_id,
      `${path}.runtime_id`,
      LIMITS.idBytes,
    ),
    label: boundedString(item.label, `${path}.label`, LIMITS.labelBytes),
    kind: boundedString(item.kind, `${path}.kind`, LIMITS.kindBytes),
    status: boundedString(item.status, `${path}.status`, LIMITS.statusBytes),
    source: runtimeSource(item.source, `${path}.source`),
    host_ids: hostIds,
    worker_creation_available: boolean(
      item.worker_creation_available,
      `${path}.worker_creation_available`,
    ),
    os: boundedString(item.os, `${path}.os`, LIMITS.kindBytes, true),
    arch: boundedString(item.arch, `${path}.arch`, LIMITS.kindBytes, true),
    diagnostics: array(
      item.diagnostics,
      `${path}.diagnostics`,
      LIMITS.diagnostics,
    ).map((entry, index) => diagnostic(entry, `${path}.diagnostics[${index}]`)),
  };
}

function trustKey(value: unknown, path: string): RuntimeTrustKeyState {
  const item = object(value, path);
  exactKeys(
    item,
    ["status"],
    [
      "public_key",
      "fingerprint",
      "revision",
      "created_at",
      "updated_at",
      "revoked_at",
    ],
    path,
  );
  const result: RuntimeTrustKeyState = {
    status: enumValue(item.status, `${path}.status`, TRUST_STATUSES),
    public_key: optionalNullableString(
      item.public_key,
      `${path}.public_key`,
      LIMITS.publicKeyBytes,
    ),
    fingerprint: optionalNullableString(
      item.fingerprint,
      `${path}.fingerprint`,
      LIMITS.fingerprintBytes,
    ),
    revision: optionalNullableRevision(item.revision, `${path}.revision`),
    created_at: optionalNullableTimestamp(
      item.created_at,
      `${path}.created_at`,
    ),
    updated_at: optionalNullableTimestamp(
      item.updated_at,
      `${path}.updated_at`,
    ),
    revoked_at: optionalNullableTimestamp(
      item.revoked_at,
      `${path}.revoked_at`,
    ),
  };

  const hasBinding = result.status !== "unconfigured";
  if (
    hasBinding &&
    (result.fingerprint == null || result.revision == null ||
      result.created_at == null || result.updated_at == null)
  ) {
    fail(
      path,
      "must include fingerprint, revision, created_at, and updated_at",
    );
  }
  if (
    !hasBinding &&
    Object.entries(result).some(([key, entry]) =>
      key !== "status" && entry != null
    )
  ) {
    fail(path, "must not include binding values while unconfigured");
  }
  if (result.status === "revoked" && result.revoked_at == null) {
    fail(`${path}.revoked_at`, "is required for a revoked key");
  }
  if (result.status === "active" && result.revoked_at != null) {
    fail(`${path}.revoked_at`, "must be absent for an active key");
  }
  return result;
}

function auditEntry(value: unknown, path: string): RuntimeTrustAuditEntry {
  const item = object(value, path);
  exactKeys(
    item,
    ["action", "actor_account_id", "revision", "at"],
    ["old_fingerprint", "new_fingerprint"],
    path,
  );
  return {
    action: enumValue(item.action, `${path}.action`, AUDIT_ACTIONS),
    actor_account_id: boundedString(
      item.actor_account_id,
      `${path}.actor_account_id`,
      LIMITS.idBytes,
    ),
    old_fingerprint: optionalNullableString(
      item.old_fingerprint,
      `${path}.old_fingerprint`,
      LIMITS.fingerprintBytes,
    ),
    new_fingerprint: optionalNullableString(
      item.new_fingerprint,
      `${path}.new_fingerprint`,
      LIMITS.fingerprintBytes,
    ),
    revision: safeRevision(item.revision, `${path}.revision`),
    at: timestamp(item.at, `${path}.at`),
  };
}

export function parseWorkspaceRuntimeList(
  value: unknown,
): WorkspaceRuntimeList {
  const response = object(value, "Runtime list response");
  exactKeys(
    response,
    ["workspace_id", "limit", "items", "source", "diagnostics"],
    [],
    "Runtime list response",
  );
  const limit = safeInteger(response.limit, "Runtime list response.limit", 0);
  if (limit > LIMITS.runtimeItems) {
    fail(
      "Runtime list response.limit",
      `must not exceed ${LIMITS.runtimeItems}`,
    );
  }
  const items = array(
    response.items,
    "Runtime list response.items",
    LIMITS.runtimeItems,
  ).map((entry, index) =>
    runtimeResource(entry, `Runtime list response.items[${index}]`)
  );
  if (items.length > limit) {
    fail("Runtime list response.items", "must not exceed the declared limit");
  }
  return {
    workspace_id: boundedString(
      response.workspace_id,
      "Runtime list response.workspace_id",
      LIMITS.idBytes,
    ),
    limit,
    items,
    source: boundedString(
      response.source,
      "Runtime list response.source",
      LIMITS.kindBytes,
    ),
    diagnostics: array(
      response.diagnostics,
      "Runtime list response.diagnostics",
      LIMITS.diagnostics,
    ).map((entry, index) =>
      diagnostic(entry, `Runtime list response.diagnostics[${index}]`)
    ),
  };
}

export function parseWorkspaceRuntimeDetail(
  value: unknown,
): WorkspaceRuntimeDetail {
  const response = object(value, "Runtime detail response");
  exactKeys(
    response,
    ["workspace_id", "runtime", "trust_key", "recent_audit"],
    ["endpoint"],
    "Runtime detail response",
  );
  return {
    workspace_id: boundedString(
      response.workspace_id,
      "Runtime detail response.workspace_id",
      LIMITS.idBytes,
    ),
    runtime: runtimeResource(
      response.runtime,
      "Runtime detail response.runtime",
    ),
    endpoint: optionalNullableString(
      response.endpoint,
      "Runtime detail response.endpoint",
      LIMITS.endpointBytes,
    ),
    trust_key: trustKey(
      response.trust_key,
      "Runtime detail response.trust_key",
    ),
    recent_audit: array(
      response.recent_audit,
      "Runtime detail response.recent_audit",
      LIMITS.auditEntries,
    ).map((entry, index) =>
      auditEntry(entry, `Runtime detail response.recent_audit[${index}]`)
    ),
  };
}

export function parseRuntimeTrustConflict(
  value: unknown,
): RuntimeTrustConflictResponse {
  const response = object(value, "Runtime trust conflict");
  exactKeys(
    response,
    ["error", "message"],
    ["current_revision", "current_fingerprint"],
    "Runtime trust conflict",
  );
  return {
    error: enumValue(
      response.error,
      "Runtime trust conflict.error",
      CONFLICT_KINDS,
    ),
    message: boundedString(
      response.message,
      "Runtime trust conflict.message",
      LIMITS.conflictMessageBytes,
    ),
    current_revision: optionalRevision(
      response.current_revision,
      "Runtime trust conflict.current_revision",
    ),
    current_fingerprint: optionalNullableString(
      response.current_fingerprint,
      "Runtime trust conflict.current_fingerprint",
      LIMITS.fingerprintBytes,
    ),
  };
}

function revisionForJson(revision: number | null): number | null {
  if (revision === null) return null;
  if (!Number.isSafeInteger(revision) || revision < 1) {
    throw new RuntimeTrustRequestError(
      "Runtime trust revision is not a safe integer",
    );
  }
  return revision;
}

async function readBoundedJson(response: Response): Promise<unknown> {
  const contentLength = response.headers.get("content-length");
  if (contentLength !== null) {
    const parsed = Number(contentLength);
    if (Number.isFinite(parsed) && parsed > LIMITS.responseBytes) {
      throw new RuntimeTrustRequestError(
        "Runtime trust response exceeds its byte limit",
      );
    }
  }
  const text = await response.text();
  if (encoder.encode(text).byteLength > LIMITS.responseBytes) {
    throw new RuntimeTrustRequestError(
      "Runtime trust response exceeds its byte limit",
    );
  }
  try {
    return JSON.parse(text) as unknown;
  } catch {
    throw new RuntimeTrustRequestError(
      "Runtime trust response is not valid JSON",
    );
  }
}

function requestErrorFrom(
  value: unknown,
  status: number,
): RuntimeTrustRequestError {
  try {
    const response = object(value, "Runtime trust error");
    exactKeys(
      response,
      ["error", "message", "diagnostics"],
      [],
      "Runtime trust error",
    );
    const diagnostics = array(
      response.diagnostics,
      "Runtime trust error.diagnostics",
      LIMITS.diagnostics,
    ).map((entry, index) =>
      diagnostic(entry, `Runtime trust error.diagnostics[${index}]`)
    );
    const message = boundedString(
      response.message,
      "Runtime trust error.message",
      LIMITS.conflictMessageBytes,
    );
    const field = diagnostics.some((entry) =>
        entry.code.startsWith("runtime_public_key_")
      )
      ? "public_key"
      : null;
    return new RuntimeTrustRequestError(message, field);
  } catch {
    return new RuntimeTrustRequestError(
      `Runtime trust request failed (${status})`,
    );
  }
}

async function finishMutation(
  response: Response,
  workspaceId: string,
  runtimeId: string,
): Promise<WorkspaceRuntimeDetail> {
  const payload = await readBoundedJson(response);
  if (response.status === 409) {
    try {
      throw new RuntimeTrustConflictError(parseRuntimeTrustConflict(payload));
    } catch (error) {
      if (error instanceof RuntimeTrustConflictError) throw error;
      throw new RuntimeTrustRequestError(
        "Runtime trust conflict response was invalid",
      );
    }
  }
  if (!response.ok) throw requestErrorFrom(payload, response.status);
  let detail: WorkspaceRuntimeDetail;
  try {
    detail = parseWorkspaceRuntimeDetail(payload);
  } catch {
    throw new RuntimeTrustRequestError("Runtime trust response was invalid");
  }
  if (
    detail.workspace_id !== workspaceId ||
    detail.runtime.runtime_id !== runtimeId
  ) {
    throw new RuntimeTrustRequestError(
      "Runtime trust response did not match the selected Runtime",
    );
  }
  return detail;
}

export async function previewRuntimePublicKeyFingerprint(
  publicKey: string,
): Promise<string> {
  const normalized = publicKey.trim();
  const prefix = "yoi-ed25519-pub:v1:";
  if (!normalized.startsWith(prefix)) {
    throw new RuntimeTrustRequestError(
      `Public key must start with ${prefix}`,
    );
  }
  const encoded = normalized.slice(prefix.length);
  if (!/^[A-Za-z0-9_-]+$/.test(encoded)) {
    throw new RuntimeTrustRequestError("Public key encoding is invalid");
  }
  const padded = encoded.replaceAll("-", "+").replaceAll("_", "/") +
    "=".repeat((4 - (encoded.length % 4)) % 4);
  let decoded: string;
  try {
    decoded = atob(padded);
  } catch {
    throw new RuntimeTrustRequestError("Public key encoding is invalid");
  }
  if (decoded.length !== 32) {
    throw new RuntimeTrustRequestError("Public key must contain 32 bytes");
  }
  const bytes = Uint8Array.from(
    decoded,
    (character) => character.charCodeAt(0),
  );
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  const hex = Array.from(digest, (byte) => byte.toString(16).padStart(2, "0"))
    .join("");
  return `sha256:${hex}`;
}

export async function putRuntimeTrustKey(
  workspaceId: string,
  runtimeId: string,
  request: PutRuntimeTrustKeyRequest,
  fetchImpl: typeof fetch = fetch,
): Promise<WorkspaceRuntimeDetail> {
  const response = await fetchImpl(
    workspaceApiPath(
      workspaceId,
      `/runtimes/${encodeURIComponent(runtimeId)}/trust-key`,
    ),
    {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        public_key: request.public_key,
        expected_revision: revisionForJson(request.expected_revision),
      }),
    },
  );
  return await finishMutation(response, workspaceId, runtimeId);
}

export async function revokeRuntimeTrustKey(
  workspaceId: string,
  runtimeId: string,
  request: RevokeRuntimeTrustKeyRequest,
  fetchImpl: typeof fetch = fetch,
): Promise<WorkspaceRuntimeDetail> {
  const response = await fetchImpl(
    workspaceApiPath(
      workspaceId,
      `/runtimes/${encodeURIComponent(runtimeId)}/trust-key`,
    ),
    {
      method: "DELETE",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        expected_revision: revisionForJson(request.expected_revision),
      }),
    },
  );
  return await finishMutation(response, workspaceId, runtimeId);
}
