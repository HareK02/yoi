import type {
  Diagnostic,
  RuntimeConnectionTestFailureKind,
  RuntimeConnectionTestResponse,
  RuntimeVerificationEvidenceSummary,
} from "$lib/generated/workspace-api";

const RESPONSE_KEYS = [
  "workspace_id",
  "runtime_id",
  "binding_revision",
  "connection_state",
  "verification",
  "checked_at",
  "status",
  "failure_kind",
  "expected_protocol_version",
  "actual_protocol_version",
  "diagnostics",
] as const;
const DIAGNOSTIC_KEYS = ["code", "severity", "message"] as const;
const FAILURE_KINDS = new Set<RuntimeConnectionTestFailureKind>([
  "authentication",
  "authorization",
  "network_unreachable",
  "timeout",
  "tls_or_transport",
  "malformed_response",
  "protocol_version_mismatch",
  "runtime_identity_mismatch",
  "configuration",
]);

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasExactKeys(
  record: Record<string, unknown>,
  expected: readonly string[],
): boolean {
  const actual = Object.keys(record).sort();
  const wanted = [...expected].sort();
  return actual.length === wanted.length &&
    actual.every((key, index) => key === wanted[index]);
}

function isBoundedString(value: unknown, max = 1024): value is string {
  return typeof value === "string" && value.length > 0 && value.length <= max;
}

function isProtocolVersion(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) >= 0;
}

function parseDiagnostic(value: unknown): Diagnostic | null {
  if (!isRecord(value) || !hasExactKeys(value, DIAGNOSTIC_KEYS)) return null;
  if (!isBoundedString(value.code, 128) || !isBoundedString(value.message)) {
    return null;
  }
  if (
    value.severity !== "info" && value.severity !== "warning" &&
    value.severity !== "error"
  ) {
    return null;
  }
  return {
    code: value.code,
    severity: value.severity,
    message: value.message,
  };
}

export function parseRuntimeConnectionTestResponse(
  value: unknown,
): RuntimeConnectionTestResponse | null {
  if (!isRecord(value) || !hasExactKeys(value, RESPONSE_KEYS)) return null;
  if (
    !isBoundedString(value.workspace_id, 256) ||
    !isBoundedString(value.runtime_id, 256) ||
    !isBoundedString(value.checked_at, 128) ||
    Number.isNaN(Date.parse(value.checked_at)) ||
    (value.status !== "compatible" && value.status !== "failed") ||
    !isProtocolVersion(value.expected_protocol_version) ||
    (value.actual_protocol_version !== null &&
      !isProtocolVersion(value.actual_protocol_version)) ||
    !Array.isArray(value.diagnostics) ||
    value.diagnostics.length > 16
  ) {
    return null;
  }
  const failureKind = value.failure_kind;
  if (
    failureKind !== null &&
    !FAILURE_KINDS.has(failureKind as RuntimeConnectionTestFailureKind)
  ) {
    return null;
  }
  const diagnostics = value.diagnostics.map(parseDiagnostic);
  if (diagnostics.some((diagnostic) => diagnostic === null)) return null;
  if (
    (value.status === "compatible" &&
      (failureKind !== null ||
        value.actual_protocol_version !== value.expected_protocol_version ||
        diagnostics.length !== 0)) ||
    (value.status === "failed" && failureKind === null)
  ) {
    return null;
  }
  const bindingRevision = value.binding_revision;
  const connectionState = parseConnectionState(value.connection_state);
  const verification = parseVerificationEvidence(value.verification);
  if (
    !isSafeRevision(bindingRevision) ||
    connectionState === null ||
    (value.verification !== null && verification === null) ||
    (verification !== null &&
      verification.binding_revision !== bindingRevision) ||
    (connectionState === "verified" && value.status !== "compatible") ||
    (connectionState === "verified" && verification !== null &&
      (verification.last_outcome !== "verified" ||
        verification.verified_at === null))
  ) {
    return null;
  }
  return {
    workspace_id: value.workspace_id,
    runtime_id: value.runtime_id,
    binding_revision: bindingRevision,
    connection_state: connectionState,
    verification,
    checked_at: value.checked_at,
    status: value.status,
    failure_kind: failureKind as RuntimeConnectionTestFailureKind | null,
    expected_protocol_version: value.expected_protocol_version,
    actual_protocol_version: value.actual_protocol_version,
    diagnostics: diagnostics as Diagnostic[],
  };
}

function parseConnectionState(
  value: unknown,
): "configured" | "verified" | "unavailable" | "revoked" | null {
  return value === "configured" || value === "verified" ||
      value === "unavailable" || value === "revoked"
    ? value
    : null;
}

function isSafeRevision(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) >= 0;
}

function parseVerificationEvidence(
  value: unknown,
): RuntimeVerificationEvidenceSummary | null {
  if (value === null) return null;
  const keys = [
    "verified_at",
    "last_checked_at",
    "last_outcome",
    "binding_revision",
    "workspace_key_id",
    "workspace_identity_revision",
    "workspace_trust_generation",
    "runtime_public_key_fingerprint",
    "runtime_identity_revision",
  ] as const;
  if (!isRecord(value) || !hasExactKeys(value, keys)) return null;
  if (
    (value.verified_at !== null &&
      (!isBoundedString(value.verified_at, 128) ||
        Number.isNaN(Date.parse(value.verified_at)))) ||
    !isBoundedString(value.last_checked_at, 128) ||
    Number.isNaN(Date.parse(value.last_checked_at)) ||
    (value.last_outcome !== "verified" &&
      value.last_outcome !== "challenge_issued" &&
      value.last_outcome !== "verification_failed" &&
      value.last_outcome !== "connectivity_failed") ||
    !isSafeRevision(value.binding_revision) ||
    !isBoundedString(value.workspace_key_id, 128) ||
    !isSafeRevision(value.workspace_identity_revision) ||
    !isSafeRevision(value.workspace_trust_generation) ||
    !isBoundedString(value.runtime_public_key_fingerprint, 128) ||
    !isSafeRevision(value.runtime_identity_revision)
  ) {
    return null;
  }
  return {
    verified_at: value.verified_at,
    last_checked_at: value.last_checked_at,
    last_outcome: value.last_outcome,
    binding_revision: value.binding_revision,
    workspace_key_id: value.workspace_key_id,
    workspace_identity_revision: value.workspace_identity_revision,
    workspace_trust_generation: value.workspace_trust_generation,
    runtime_public_key_fingerprint: value.runtime_public_key_fingerprint,
    runtime_identity_revision: value.runtime_identity_revision,
  };
}

export async function testRuntimeConnection(
  workspaceId: string,
  runtimeId: string,
  fetchImpl: typeof fetch = fetch,
): Promise<RuntimeConnectionTestResponse> {
  const response = await fetchImpl(
    `/api/w/${encodeURIComponent(workspaceId)}/runtimes/${
      encodeURIComponent(runtimeId)
    }/connection-tests`,
    { method: "POST" },
  );
  if (!response.ok) {
    throw new Error(`Connection test failed (${response.status})`);
  }
  const parsed = parseRuntimeConnectionTestResponse(await response.json());
  if (!parsed) {
    throw new Error("Connection test returned an invalid response");
  }
  if (parsed.workspace_id !== workspaceId || parsed.runtime_id !== runtimeId) {
    throw new Error(
      "Connection test response did not match the selected Runtime",
    );
  }
  return parsed;
}
