import type {
  PermissionOperationDescriptor,
  PermissionResource,
  ScopedPermission,
  SubjectPermissionInterruption,
  SubjectPermissions,
  SubjectPermissionsMutationResponse,
  SubjectPermissionsResponse,
  SubjectPermissionsUpdate,
} from "#lib/generated/memory-api.ts";
import { readBoundedJson, workspaceApiPath } from "#lib/workspace/api/http.ts";

export type { PermissionResource, ScopedPermission };
export type Policy = SubjectPermissions;
export type PolicyUpdate = SubjectPermissionsUpdate;
export type Operation = Omit<PermissionOperationDescriptor, "availability"> & {
  available: boolean;
  unavailable_reason: string | null;
};
export type PermissionView = Omit<SubjectPermissionsResponse, "operations"> & {
  operations: Operation[];
};
export type Interruption = SubjectPermissionInterruption;
export type Mutation = SubjectPermissionsMutationResponse;
export type Lane = "execution" | "delegation";

const MAX_BYTES = 1_048_576;
const fields = {
  workspace: ["workspace_id"],
  worker_operation: ["workspace_id"],
  plugin_installation: ["workspace_id", "installation_id"],
  ticket: ["workspace_id", "ticket_id"],
  objective: ["workspace_id", "objective_id"],
  merge_request: ["workspace_id", "merge_request_id"],
  subject: ["workspace_id", "subject_id"],
  drive_entry: ["workspace_id", "node_id"],
  drive_request: ["workspace_id", "request_id"],
  repository: ["workspace_id", "repository_key"],
  workdir_attachment: ["workspace_id", "connection_id"],
  workspace_config: ["workspace_id", "path", "recursive"],
  network: ["url"],
  sub_worker: ["runtime_id", "worker_id", "name"],
  feature: [],
  workdir: ["working_directory_id", "path", "recursive"],
  worker: ["runtime_id", "worker_id"],
  job: ["workspace_id", "job_id"],
} satisfies Record<PermissionResource["kind"], string[]>;
function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("Invalid permission response");
  }
  return value as Record<string, unknown>;
}
function text(value: unknown): string {
  if (typeof value !== "string" || value.length > 16_384) {
    throw new Error("Invalid permission text");
  }
  return value;
}
function bool(value: unknown): boolean {
  if (typeof value !== "boolean") throw new Error("Invalid permission flag");
  return value;
}
function list<T>(value: unknown, parse: (item: unknown) => T): T[] {
  if (!Array.isArray(value) || value.length > 256) {
    throw new Error("Invalid permission list");
  }
  return value.map(parse);
}
export function parseResource(value: unknown): PermissionResource {
  const r = object(value);
  const kind = text(r.kind) as PermissionResource["kind"];
  if (!Object.hasOwn(fields, kind)) throw new Error("Unknown resource kind");
  const keys: string[] = fields[kind];
  if (
    Object.keys(r).some((key) => key !== "kind" && !keys.includes(key)) ||
    keys.some((key) => !(key in r))
  ) throw new Error("Invalid resource fields");
  for (const key of keys) key === "recursive" ? bool(r[key]) : text(r[key]);
  return r as PermissionResource;
}
export function parsePermission(value: unknown): ScopedPermission {
  const p = object(value);
  return { operation: text(p.operation), resource: parseResource(p.resource) };
}
export function parsePolicy(value: unknown): Policy {
  const p = object(value);
  return {
    change_id: text(p.change_id),
    execution: list(p.execution, parsePermission),
    delegation: list(p.delegation, parsePermission),
  };
}
export function parseView(value: unknown): PermissionView {
  const v = object(value);
  return {
    permissions: v.permissions === null ? null : parsePolicy(v.permissions),
    can_manage: bool(v.can_manage),
    operations: list(v.operations, (value) => {
      const o = object(value);
      const availability = object(o.availability);
      if (
        availability.kind !== "available" && availability.kind !== "unavailable"
      ) throw new Error("Unknown operation availability");
      return {
        operation: text(o.operation),
        label: text(o.label),
        description: text(o.description),
        resource_kinds: list(o.resource_kinds, (value) => {
          const kind = text(value) as PermissionResource["kind"];
          if (!Object.hasOwn(fields, kind)) {
            throw new Error("Unknown resource kind");
          }
          return kind;
        }),
        available: availability.kind === "available",
        unavailable_reason: availability.kind === "unavailable"
          ? text(availability.reason)
          : null,
      };
    }),
  };
}
export class PermissionHttpError extends Error {
  constructor(public status: number) {
    super(`Permission request rejected (HTTP ${status}).`);
  }
}
const path = (workspaceId: string, subjectId: string) =>
  workspaceApiPath(
    workspaceId,
    `/subjektiv/subjects/${encodeURIComponent(subjectId)}/permissions`,
  );
export async function readPermissions(
  fetchFn: typeof fetch,
  workspaceId: string,
  subjectId: string,
): Promise<PermissionView> {
  const response = await fetchFn(path(workspaceId, subjectId));
  if (!response.ok) {
    await response.body?.cancel();
    throw new PermissionHttpError(response.status);
  }
  return parseView(await readBoundedJson(response, MAX_BYTES));
}
export async function writePermissions(
  fetchFn: typeof fetch,
  workspaceId: string,
  subjectId: string,
  request: PolicyUpdate,
): Promise<Mutation> {
  const response = await fetchFn(path(workspaceId, subjectId), {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(request),
  });
  if (!response.ok) {
    await response.body?.cancel();
    throw new PermissionHttpError(response.status);
  }
  const v = object(await readBoundedJson(response, MAX_BYTES));
  return {
    permissions: parsePolicy(v.permissions),
    interruptions: list(v.interruptions, (value) => {
      const i = object(value);
      return {
        runtime_id: text(i.runtime_id),
        worker_id: text(i.worker_id),
        cancellation_accepted: bool(i.cancellation_accepted),
        backend_command_sessions_closed: bool(
          i.backend_command_sessions_closed,
        ),
        diagnostics: list(i.diagnostics, text),
      };
    }),
  };
}
export function permissionKey(permission: ScopedPermission): string {
  return JSON.stringify([
    permission.operation,
    Object.entries(permission.resource).sort(([a], [b]) => a.localeCompare(b)),
  ]);
}
export function policyDiff(
  before: ScopedPermission[],
  after: ScopedPermission[],
) {
  return {
    added: after.filter((p) =>
      !before.some((b) => permissionKey(b) === permissionKey(p))
    ),
    removed: before.filter((p) =>
      !after.some((a) => permissionKey(a) === permissionKey(p))
    ),
  };
}
export function resourceLabel(resource: PermissionResource): string {
  const labels: Record<string, string> = {
    workspace: "Current workspace",
    worker_operation: "Worker operation use (target control grant required)",
    plugin_installation: "Plugin installation",
    subject: "Subject",
    ticket: "Ticket",
    objective: "Objective",
    merge_request: "Merge request",
    workdir: "Workdir",
    workspace_config: "Workspace configuration",
    network: "URL",
    drive_entry: "Drive entry",
    drive_request: "Drive request",
    repository: "Repository",
    workdir_attachment: "Workdir connection",
    feature: "MCP activation / legacy feature",
    worker: "Legacy Worker target",
    sub_worker: "Internal SubWorker",
    job: "Job",
  };
  const identifiers = Object.entries(resource).filter(([key]) =>
    !["kind", "workspace_id", "recursive"].includes(key)
  ).map(([_, value]) => String(value));
  return `${labels[resource.kind]}${
    identifiers.length ? ` · ${identifiers.join(" / ")}` : ""
  }${
    "recursive" in resource && resource.recursive
      ? " (including descendants)"
      : ""
  }`;
}
// 保存値の正本ではなく、同じ操作の完全一致再送に必要な入力だけをtab内に保持する。
export function attemptKey(workspaceId: string, subjectId: string): string {
  return `yoi.subject-permissions:${JSON.stringify([workspaceId, subjectId])}`;
}
export function readAttempt(
  storage: Pick<Storage, "getItem">,
  workspaceId: string,
  subjectId: string,
): PolicyUpdate | null {
  const raw = storage.getItem(attemptKey(workspaceId, subjectId));
  if (raw === null) return null;
  if (raw.length > MAX_BYTES) {
    throw new Error(
      "Saved permission request is unreadable. Do not start another save.",
    );
  }
  const v = object(JSON.parse(raw));
  return {
    operation_id: text(v.operation_id),
    expected_change_id: v.expected_change_id === null
      ? null
      : text(v.expected_change_id),
    execution: list(v.execution, parsePermission),
    delegation: list(v.delegation, parsePermission),
  };
}
export function makeResource(
  kind: PermissionResource["kind"],
  workspaceId: string,
  subjectId: string,
  inputs: Record<string, string>,
  recursive: boolean,
): PermissionResource {
  const r: Record<string, unknown> = { kind };
  for (const key of fields[kind]) {
    r[key] = key === "workspace_id"
      ? workspaceId
      : key === "recursive"
      ? recursive
      : key === "subject_id"
      ? (inputs[key]?.trim() || subjectId)
      : inputs[key]?.trim() ?? "";
    if (
      typeof r[key] === "string" && !(r[key] as string).length && key !== "path"
    ) throw new Error("Enter the required target resource.");
  }
  if (kind === "workdir" && !r.path) r.path = ".";
  return parseResource(r);
}
export function resourceFields(kind: PermissionResource["kind"]): string[] {
  return fields[kind].filter((key) =>
    !["workspace_id", "recursive"].includes(key)
  );
}
export const fieldLabel: Record<string, string> = {
  installation_id: "Plugin installation identifier",
  ticket_id: "Ticket identifier",
  objective_id: "Objective identifier",
  merge_request_id: "Merge request identifier",
  subject_id: "Subject identifier (empty means this Subject)",
  node_id: "Drive entry number",
  request_id: "Drive request identifier",
  repository_key: "Repository key",
  connection_id: "Workdir connection identifier",
  path: "Relative path",
  url: "Exact HTTP(S) URL",
  working_directory_id: "Workdir identifier",
  job_id: "Job identifier",
  runtime_id: "Runtime identifier",
  worker_id: "Worker identifier",
  name: "Internal SubWorker name",
};
