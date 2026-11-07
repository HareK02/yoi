import type {
  OriginStep,
  PermissionAssignment,
  PermissionCatalog,
  PermissionDiagnostic,
  PermissionReference,
  PermissionResolution,
} from "#lib/generated/feature-permissions.generated.ts";
import type {
  ConfigurationMigrationDiagnostic,
  ConfigurationSourceResponse,
  ConfigurationSubjectPolicy,
} from "#lib/generated/configuration-adoption.generated.ts";
import type {
  ScopedPermission,
  SubjectPermissions,
} from "#lib/generated/memory-api.ts";
import { readBoundedJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
import { parseConfigurationSourceResponse } from "#lib/workspace/config-source/api.ts";

export type Lane = "execution" | "delegation";
export type Policy = ConfigurationSubjectPolicy;
export type PermissionView = {
  source: ConfigurationSourceResponse;
  policy: Policy | null;
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
export function parsePolicy(value: unknown, workspaceId: string): Policy {
  const policy = record(value, [
    "subject_id",
    "change_id",
    "execution",
    "delegation",
    "execution_resolution",
    "delegation_resolution",
    "legacy_constraints",
  ]);
  return {
    subject_id: text(policy.subject_id),
    change_id: text(policy.change_id),
    execution: parseAssignment(policy.execution),
    delegation: parseAssignment(policy.delegation),
    execution_resolution: parseResolution(
      policy.execution_resolution,
      workspaceId,
    ),
    delegation_resolution: parseResolution(
      policy.delegation_resolution,
      workspaceId,
    ),
    legacy_constraints: parseLegacy(policy.legacy_constraints),
  };
}
export function parseSource(
  value: unknown,
  workspaceId: string,
): ConfigurationSourceResponse {
  const editing = parseConfigurationSourceResponse(value);
  const raw = object(value);
  const catalog = parseCatalog(raw.catalog);
  if (catalog.workspace !== workspaceId) return invalid();
  const policies = list(
    raw.policies,
    (policy) => parsePolicy(policy, workspaceId),
  );
  if (
    new Set(policies.map((policy) => policy.subject_id)).size !==
      policies.length
  ) return invalid();
  const migration_diagnostics = list(
    raw.migration_diagnostics,
    (value): ConfigurationMigrationDiagnostic => {
      const item = object(value);
      if (item.kind === "converted") {
        record(value, ["kind", "subject_id", "operation"]);
        return {
          kind: "converted" as const,
          subject_id: text(item.subject_id),
          operation: text(item.operation),
        };
      }
      if (
        item.kind !== "domain_constraint" &&
        item.kind !== "explicit_configuration_required"
      ) return invalid();
      record(value, ["kind", "subject_id", "permission"]);
      return {
        kind: item.kind,
        subject_id: text(item.subject_id),
        permission: parseLegacyPermission(item.permission),
      };
    },
  );
  return { ...editing, catalog, policies, migration_diagnostics };
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
export async function readPolicySource(
  fetchFn: typeof fetch,
  workspaceId: string,
): Promise<ConfigurationSourceResponse> {
  return parseSource(
    await request(fetchFn, workspaceApiPath(workspaceId, "/config/adoption")),
    workspaceId,
  );
}
export async function readPermissions(
  fetchFn: typeof fetch,
  workspaceId: string,
  subjectId: string,
): Promise<PermissionView> {
  const source = await readPolicySource(fetchFn, workspaceId);
  const policy =
    source.policies.find((policy) => policy.subject_id === subjectId) ?? null;
  // The Server binds policy, catalog and both resolutions to this one source
  // response. Arbitrary-assignment validation is not a saved-policy observation.
  return { source, policy };
}
export function referenceLabel(reference: PermissionReference): string {
  return reference.kind === "set"
    ? `Set · ${reference.value}`
    : `Namespace · ${reference.value.namespace} · ${
      reference.value.keys.join(", ") || "(no keys)"
    }`;
}
