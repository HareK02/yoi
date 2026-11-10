import type {
  OriginStep,
  PermissionAssignment,
  PermissionCatalog,
  PermissionDiagnostic,
  PermissionReference,
  PermissionResolution,
} from "#lib/generated/feature-permissions.generated.ts";
import type {
  CurrentSubjectPolicy,
  ScopedPermission,
  SubjectPermissionPolicyAction,
  SubjectPermissionPolicyMutation,
  SubjectPermissionPolicyMutationResponse,
  SubjectPermissionPolicyResponse,
  SubjectPermissions,
} from "#lib/generated/memory-api.ts";
import { readBoundedJson, workspaceApiPath } from "#lib/workspace/api/http.ts";

export type {
  CurrentSubjectPolicy,
  SubjectPermissionPolicyAction,
  SubjectPermissionPolicyMutation,
  SubjectPermissionPolicyMutationResponse,
  SubjectPermissionPolicyResponse,
};

export type Lane = "execution" | "delegation";
export type Policy = CurrentSubjectPolicy;
export type PermissionView = SubjectPermissionPolicyResponse & {
  readOnly: boolean;
};
const MAX_BYTES = 8 * 1024 * 1024;
function invalid(): never {
  throw new Error("Invalid permission response");
}
function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return invalid();
  }
  return value as Record<string, unknown>;
}
function record(value: unknown, keys: string[]): Record<string, unknown> {
  const result = object(value);
  if (
    keys.some((key) => !(key in result)) ||
    Object.keys(result).some((key) => !keys.includes(key))
  ) return invalid();
  return result;
}
function text(value: unknown): string {
  if (typeof value !== "string" || value.length > 16384) return invalid();
  return value;
}
function bool(value: unknown): boolean {
  if (typeof value !== "boolean") return invalid();
  return value;
}
function list<T>(value: unknown, parse: (value: unknown) => T): T[] {
  if (!Array.isArray(value) || value.length > 4096) return invalid();
  return value.map(parse);
}
export function parseReference(value: unknown): PermissionReference {
  const ref = record(value, ["kind", "value"]);
  if (ref.kind === "set") return { kind: "set", value: text(ref.value) };
  if (ref.kind !== "namespace") return invalid();
  const namespace = record(ref.value, ["namespace", "keys"]);
  return {
    kind: "namespace",
    value: {
      namespace: text(namespace.namespace),
      keys: list(namespace.keys, text),
    },
  };
}
export function parseAssignment(value: unknown): PermissionAssignment {
  const assignment = record(value, ["grants", "exclusions", "revoked"]);
  return {
    grants: list(assignment.grants, parseReference),
    exclusions: list(assignment.exclusions, parseReference),
    revoked: bool(assignment.revoked),
  };
}
export function parseCatalog(value: unknown): PermissionCatalog {
  const catalog = record(value, ["workspace", "namespaces"]);
  return {
    workspace: text(catalog.workspace),
    namespaces: list(catalog.namespaces, (value) => {
      const namespace = record(value, [
        "namespace",
        "feature",
        "enabled",
        "declaration",
      ]);
      const declaration = record(namespace.declaration, [
        "permissions",
        "sets",
      ]);
      return {
        namespace: text(namespace.namespace),
        feature: text(namespace.feature),
        enabled: bool(namespace.enabled),
        declaration: {
          permissions: list(declaration.permissions, (value) => {
            const permission = record(value, [
              "key",
              "description",
              "children",
            ]);
            return {
              key: text(permission.key),
              description: text(permission.description),
              children: list(permission.children, text),
            };
          }),
          sets: list(declaration.sets, (value) => {
            const set = record(value, ["set", "children"]);
            return {
              set: text(set.set),
              children: list(set.children, parseReference),
            };
          }),
        },
      };
    }),
  };
}
// Retained domain restrictions are read-only migration evidence, never inputs on a key.
function parseLegacyPermission(value: unknown): ScopedPermission {
  const permission = record(value, ["operation", "resource"]);
  const resource = object(permission.resource);
  text(resource.kind);
  for (const value of Object.values(resource)) {
    if (typeof value !== "boolean") text(value);
  }
  return {
    operation: text(permission.operation),
    resource,
  } as ScopedPermission;
}
function parseLegacy(value: unknown): SubjectPermissions | null {
  if (value === null) return null;
  const policy = record(value, ["change_id", "execution", "delegation"]);
  return {
    change_id: text(policy.change_id),
    execution: list(policy.execution, parseLegacyPermission),
    delegation: list(policy.delegation, parseLegacyPermission),
  };
}
export function parsePolicy(value: unknown): Policy {
  const raw = object(value);
  const hasEncoding = "reference_encoding" in raw;
  const policy = record(value, [
    "change_id",
    "execution",
    "delegation",
    "legacy_constraints",
    ...(hasEncoding ? ["reference_encoding"] : []),
  ]);
  const encoding = policy.reference_encoding;
  const result: Policy = {
    change_id: text(policy.change_id),
    execution: parseAssignment(policy.execution),
    delegation: parseAssignment(policy.delegation),
    legacy_constraints: parseLegacy(policy.legacy_constraints),
  };
  if (hasEncoding) {
    if (encoding !== "legacy_subject_v1" && encoding !== "feature_sets_v2") {
      return invalid();
    }
    result.reference_encoding = encoding;
  }
  return result;
}
function parsePolicyState(
  response: Record<string, unknown>,
): SubjectPermissionPolicyMutationResponse {
  const change_id = response.change_id === null ? null : text(response.change_id);
  const policy = response.policy === null ? null : parsePolicy(response.policy);
  if (policy !== null && policy.change_id !== change_id) return invalid();
  return { change_id, policy };
}
export function parsePermissionMutationResponse(
  value: unknown,
): SubjectPermissionPolicyMutationResponse {
  return parsePolicyState(record(value, ["change_id", "policy"]));
}
export function parsePermissionResponse(
  value: unknown,
  workspaceId: string,
): PermissionView {
  const response = record(value, [
    "change_id",
    "policy",
    "can_manage",
    "catalog",
    "execution_resolution",
    "delegation_resolution",
  ]);
  const { change_id, policy } = parsePolicyState(response);
  const can_manage = bool(response.can_manage);
  const catalog = parseCatalog(response.catalog);
  if (catalog.workspace !== workspaceId) return invalid();
  return {
    change_id,
    policy,
    can_manage,
    catalog,
    execution_resolution: parseResolution(
      response.execution_resolution,
      workspaceId,
    ),
    delegation_resolution: parseResolution(
      response.delegation_resolution,
      workspaceId,
    ),
    // Unset is not a default assignment. Owners may explicitly create one using
    // the response change_id, which may identify a removal tombstone.
    // Legacy stored references remain read-only; the client never migrates them.
    readOnly: !can_manage ||
      (policy !== null && policy.reference_encoding !== "feature_sets_v2"),
  };
}
export function parseResolution(
  value: unknown,
  workspaceId: string,
): PermissionResolution {
  const resolution = record(value, ["workspace", "allowed", "diagnostics"]);
  const workspace = text(resolution.workspace);
  if (workspace !== workspaceId) return invalid();
  const entries = Object.entries(object(resolution.allowed));
  if (entries.length > 4096) return invalid();
  const allowed = Object.fromEntries(
    entries.map(([key, paths]) => [
      text(key),
      list(paths, (path) =>
        list(path, (value): OriginStep => {
          const step = record(value, ["kind", "value"]);
          if (
            step.kind !== "set" && step.kind !== "feature" &&
            step.kind !== "key"
          ) return invalid();
          return { kind: step.kind, value: text(step.value) };
        })),
    ]),
  );
  const diagnostics = list(
    resolution.diagnostics,
    (value): PermissionDiagnostic => {
      const item = object(value);
      if (item.kind === "unset" || item.kind === "revoked") {
        record(value, ["kind"]);
        return { kind: item.kind };
      }
      if (
        item.kind !== "unknown_set" && item.kind !== "unknown_namespace" &&
        item.kind !== "unknown_key" && item.kind !== "disabled_feature" &&
        item.kind !== "excluded"
      ) return invalid();
      record(value, ["kind", "value"]);
      return { kind: item.kind, value: text(item.value) };
    },
  );
  return { workspace, allowed, diagnostics };
}
export class PermissionHttpError extends Error {
  constructor(public status: number) {
    super(`Permission request rejected (HTTP ${status}).`);
  }
}
async function request(
  fetchFn: typeof fetch,
  path: string,
): Promise<unknown> {
  const response = await fetchFn(path);
  if (!response.ok) {
    await response.body?.cancel();
    throw new PermissionHttpError(response.status);
  }
  return await readBoundedJson(response, MAX_BYTES);
}
export function permissionPolicyPath(
  workspaceId: string,
  subjectId: string,
): string {
  return workspaceApiPath(
    workspaceId,
    `/subjektiv/subjects/${encodeURIComponent(subjectId)}/permission-policy`,
  );
}
export async function readPermissions(
  fetchFn: typeof fetch,
  workspaceId: string,
  subjectId: string,
): Promise<PermissionView> {
  return parsePermissionResponse(
    await request(fetchFn, permissionPolicyPath(workspaceId, subjectId)),
    workspaceId,
  );
}
function parseMutation(value: unknown): SubjectPermissionPolicyMutation {
  const mutation = record(value, [
    "operation_id",
    "expected_change_id",
    "action",
  ]);
  const operation_id = text(mutation.operation_id);
  if (!operation_id.trim()) return invalid();
  const expected_change_id = mutation.expected_change_id === null
    ? null
    : text(mutation.expected_change_id);
  const action = object(mutation.action);
  if (action.kind === "remove") {
    record(action, ["kind"]);
    return { operation_id, expected_change_id, action: { kind: "remove" } };
  }
  if (action.kind !== "replace") return invalid();
  record(action, ["kind", "execution", "delegation"]);
  return {
    operation_id,
    expected_change_id,
    action: {
      kind: "replace",
      execution: parseAssignment(action.execution),
      delegation: parseAssignment(action.delegation),
    },
  };
}
export class PermissionMutationUnknownError extends Error {
  constructor(
    public readonly retry: (
      fetchFn?: typeof fetch,
    ) => Promise<SubjectPermissionPolicyMutationResponse>,
  ) {
    super(
      "Subject policy save outcome unknown. Retain and retry the same request; do not create a new operation.",
    );
  }
}
/**
 * CAS uses the observed response.change_id, including removal tombstones.
 * Never derive null solely from policy === null; only an observed null change_id
 * represents a state with no change identity.
 * On conflict, reread and let the caller review a NEW explicit mutation. Never
 * rebase automatically. On unknown outcome, error.retry() retains the exact URL,
 * operation ID and serialized request, even if the caller later edits its draft.
 */
export async function savePermissions(
  fetchFn: typeof fetch,
  workspaceId: string,
  subjectId: string,
  mutation: SubjectPermissionPolicyMutation,
): Promise<SubjectPermissionPolicyMutationResponse> {
  const path = permissionPolicyPath(workspaceId, subjectId);
  const body = JSON.stringify(parseMutation(mutation));
  async function send(
    transport: typeof fetch,
    retrying: boolean,
  ): Promise<SubjectPermissionPolicyMutationResponse> {
    try {
      const response = await transport(path, {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body,
      });
      if (!response.ok) {
        // Body cancellation cannot turn an authoritative first refusal into an
        // unknown result. A refusal on a retry never resolves the earlier send.
        await response.body?.cancel().catch(() => {});
        throw new PermissionHttpError(response.status);
      }
      return parsePermissionMutationResponse(
        await readBoundedJson(response, MAX_BYTES),
      );
    } catch (error) {
      if (
        !retrying && error instanceof PermissionHttpError &&
        [400, 401, 403, 404, 409, 422].includes(error.status)
      ) throw error;
      throw new PermissionMutationUnknownError((nextFetch = transport) =>
        send(nextFetch, true)
      );
    }
  }
  return await send(fetchFn, false);
}
export function referenceLabel(reference: PermissionReference): string {
  return reference.kind === "set"
    ? `Set · ${reference.value}`
    : `Namespace · ${reference.value.namespace} · ${
      reference.value.keys.join(", ") || "(no keys)"
    }`;
}
