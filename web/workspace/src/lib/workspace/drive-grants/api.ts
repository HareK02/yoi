import type {
  DriveAccess,
  DriveApiError,
  DriveGrantCreateRequest,
  DriveGrantListResponse,
  DriveGrantResponse,
} from "#lib/generated/drive-api.ts";
import { readBoundedJson, workspaceApiPath } from "#lib/workspace/api/http.ts";

// Owner-controlled grant transport, separate from ordinary Drive content access.
export type { DriveAccess, DriveGrantCreateRequest, DriveGrantResponse };
export class DriveGrantError extends Error {
  constructor(message: string, readonly outcome: "not_committed" | "unknown") {
    super(message);
    this.name = "DriveGrantError";
  }
}
function object(
  value: unknown,
  keys: readonly string[],
): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("Invalid Drive grant response");
  }
  if (Object.keys(value).some((key) => !keys.includes(key))) {
    throw new Error("Unexpected Drive grant response field");
  }
  return value as Record<string, unknown>;
}
function text(value: unknown): string {
  if (typeof value !== "string" || value.length > 4096) {
    throw new Error("Invalid Drive grant string");
  }
  return value;
}
function nullableText(value: unknown): string | null {
  return value === null ? null : text(value);
}
function decimal(value: unknown): string {
  const id = text(value);
  if (!/^[1-9][0-9]*$/.test(id) || BigInt(id) > 9223372036854775807n) {
    throw new Error("Invalid Drive grant ID");
  }
  return id;
}
export function parseDriveGrantResponse(value: unknown): DriveGrantResponse {
  const v = object(value, [
    "grant_id",
    "workspace_id",
    "runtime_id",
    "worker_id",
    "access",
    "revoked",
    "created_by",
    "created_at",
    "revoked_by",
    "revoked_at",
  ]);
  if (
    (v.access !== "read_only" && v.access !== "read_write") ||
    typeof v.revoked !== "boolean"
  ) throw new Error("Invalid Drive grant access or state");
  return {
    grant_id: decimal(v.grant_id),
    workspace_id: text(v.workspace_id),
    runtime_id: text(v.runtime_id),
    worker_id: text(v.worker_id),
    access: v.access,
    revoked: v.revoked,
    created_by: text(v.created_by),
    created_at: text(v.created_at),
    revoked_by: nullableText(v.revoked_by),
    revoked_at: nullableText(v.revoked_at),
  };
}
export function parseDriveGrantListResponse(
  value: unknown,
): DriveGrantListResponse {
  const v = object(value, ["grants", "next_after"]);
  if (!Array.isArray(v.grants) || v.grants.length > 200) {
    throw new Error("Invalid Drive grant page");
  }
  return {
    grants: v.grants.map(parseDriveGrantResponse),
    next_after: v.next_after === null ? null : decimal(v.next_after),
  };
}
export function parseDriveGrantError(value: unknown): DriveApiError {
  const v = object(value, ["code", "classification", "message"]);
  const codes = [
    "denied",
    "not_found",
    "conflict",
    "invalid",
    "limit",
    "storage_unavailable",
    "outcome_unknown",
  ] as const;
  const code = codes.find((code) => code === v.code);
  if (
    !code ||
    (v.classification !== "not_committed" && v.classification !== "unknown") ||
    (code === "outcome_unknown" && v.classification !== "unknown")
  ) throw new Error("Invalid Drive error");
  return { code, classification: v.classification, message: text(v.message) };
}
async function request<T>(
  workspaceId: string,
  path: string,
  init: RequestInit,
  parse: (value: unknown) => T,
  mutation = false,
): Promise<T> {
  try {
    // Browser same-origin authentication only; Backend rechecks current authority.
    const response = await fetch(workspaceApiPath(workspaceId, path), {
      ...init,
      credentials: "same-origin",
      cache: "no-store",
      redirect: "error",
    });
    const payload = await readBoundedJson(response, 1024 * 1024);
    if (!response.ok) {
      const error = parseDriveGrantError(payload);
      throw new DriveGrantError(error.message, error.classification);
    }
    return parse(payload);
  } catch (error) {
    if (error instanceof DriveGrantError) throw error;
    throw new DriveGrantError(
      mutation
        ? "Drive grant outcome unknown. Refresh grants before deciding on another action; do not blindly retry."
        : "Drive grants could not be loaded. Refresh to try reading them again.",
      mutation ? "unknown" : "not_committed",
    );
  }
}
export async function listDriveGrants(
  workspaceId: string,
): Promise<DriveGrantResponse[]> {
  const grants: DriveGrantResponse[] = [];
  let after: string | null = null;
  const ids = new Set<string>();
  // Fetch every page before claiming that a Worker has no active grant. Bound
  // malformed/cyclic cursors and unexpectedly large collections, never truncate.
  for (let page = 0; page < 100; page++) {
    const query = new URLSearchParams({ limit: "200" });
    if (after !== null) query.set("after", after);
    const result = await request(
      workspaceId,
      `/drive/grants?${query}`,
      {},
      parseDriveGrantListResponse,
    );
    let previousId = after;
    for (const grant of result.grants) {
      if (
        grant.workspace_id !== workspaceId || ids.has(grant.grant_id) ||
        (previousId !== null && BigInt(grant.grant_id) <= BigInt(previousId))
      ) {
        throw new DriveGrantError(
          "Invalid Drive grant page identity or order. Refresh grants.",
          "not_committed",
        );
      }
      previousId = grant.grant_id;
      ids.add(grant.grant_id);
      grants.push(grant);
    }
    if (result.next_after === null) return grants;
    if (
      !result.grants.length ||
      result.next_after !== result.grants.at(-1)?.grant_id ||
      (after !== null && BigInt(result.next_after) <= BigInt(after))
    ) {
      throw new DriveGrantError(
        "Invalid Drive grant cursor. Refresh grants.",
        "not_committed",
      );
    }
    after = result.next_after;
  }
  throw new DriveGrantError(
    "Drive grant list is too large to load completely. No complete grant state is available.",
    "not_committed",
  );
}
export function createDriveGrant(
  workspaceId: string,
  input: DriveGrantCreateRequest,
): Promise<DriveGrantResponse> {
  return request(workspaceId, "/drive/grants", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  }, (payload) => {
    const result = parseDriveGrantResponse(payload);
    if (
      result.workspace_id !== workspaceId ||
      result.runtime_id !== input.runtime_id ||
      result.worker_id !== input.worker_id || result.access !== input.access ||
      result.revoked
    ) throw new Error("Drive grant identity mismatch");
    return result;
  }, true);
}
export function revokeDriveGrant(
  workspaceId: string,
  grant: DriveGrantResponse,
): Promise<DriveGrantResponse> {
  return request(
    workspaceId,
    `/drive/grants/${encodeURIComponent(grant.grant_id)}`,
    { method: "DELETE" },
    (payload) => {
      const result = parseDriveGrantResponse(payload);
      if (
        result.workspace_id !== workspaceId ||
        result.grant_id !== grant.grant_id ||
        result.runtime_id !== grant.runtime_id ||
        result.worker_id !== grant.worker_id || !result.revoked
      ) throw new Error("Drive grant identity mismatch");
      return result;
    },
    true,
  );
}
