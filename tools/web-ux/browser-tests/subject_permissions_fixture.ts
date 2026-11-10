/**
 * Browser UXだけの合成HTTP状態。実Backend、認証、認可、resolver、取消の実装ではない。
 * 実Backendの結合証拠は、このbrowser fixtureと分けて保持する。
 */
import type {
  PermissionAssignment,
  PermissionCatalog,
  PermissionReference,
  PermissionResolution,
} from "../../../web/workspace/src/lib/generated/feature-permissions.generated.ts";
import type {
  CurrentSubjectPolicy,
  SubjectPermissionPolicyResponse,
} from "../../../web/workspace/src/lib/workspace/memory/permissions.ts";

export const permissionFixtureSubjects = [
  ["permissions-empty", "Subject policy unset"],
  ["permissions-edit", "Release coordination"],
  ["permissions-legacy", "Subject with retained domain restrictions"],
  ["permissions-reader", "Read-only Subject policy"],
  ["permissions-unavailable", "Unavailable Subject policy"],
  ["permissions-explicit-empty", "Explicitly empty Subject assignments"],
  ["permissions-removed", "Removed Subject policy with a tombstone"],
  ["permissions-revoked", "Revoked Subject assignments"],
  ["permissions-excluded", "Subject assignments with exclusions"],
  ["permissions-longnames", "Subject policy with deliberately long set, namespace and operation names"],
  ["permissions-diagnostics", "Unresolved and disabled permission references"],
  ["permissions-anonymous", "Subject policy requires authentication"],
  ["permissions-forbidden", "Subject policy access denied"],
] as const;

export type FixturePermission =
  NonNullable<CurrentSubjectPolicy["legacy_constraints"]>["execution"][number];
export type FixturePolicy = CurrentSubjectPolicy;
const workspaceId = "memory-review";
export const fixtureLegacyPermission: FixturePermission = {
  operation: "worker.notify",
  resource: { kind: "worker", runtime_id: "legacy-runtime", worker_id: "legacy-recipient" },
};
export const fixtureLongNames = {
  set: `work-execution-${"deliberately-long-set-identity-".repeat(5)}`,
  namespace: `ticket-${"deliberately-long-namespace-identity-".repeat(5)}`,
  feature: `feature-${"deliberately-long-feature-identity-".repeat(5)}`,
  key: `ticket.${"deliberately-long-operation-segment.".repeat(8)}read`,
};
const namespace = (name: string, keys: string[]): PermissionReference => ({
  kind: "namespace",
  value: { namespace: name, keys },
});
const set = (value: string): PermissionReference => ({ kind: "set", value });
const assignment = (
  grants: PermissionReference[] = [],
  exclusions: PermissionReference[] = [],
  revoked = false,
): PermissionAssignment => ({ grants, exclusions, revoked });

// 小さな合成catalogと明示的な解決結果だけを使い、製品resolverを再実装しない。
const catalog: PermissionCatalog = {
  workspace: workspaceId,
  namespaces: [
    {
      namespace: "ticket",
      feature: "ticket",
      enabled: true,
      declaration: {
        permissions: [
          {
            key: "ticket.operations",
            description: "Explicitly declared ticket operation children.",
            children: ["ticket.read", "ticket.comment"],
          },
          {
            key: "ticket.read",
            description: "Read a ticket, subject to its domain restrictions.",
            children: [],
          },
          {
            key: "ticket.comment",
            description: "Append a comment; no assignment or completion authority.",
            children: [],
          },
        ],
        sets: [
          { set: "conversation", children: [namespace("ticket", ["ticket.read"])] },
          { set: "work-execution", children: [namespace("ticket", ["ticket.operations"])] },
        ],
      },
    },
    {
      namespace: "worker",
      feature: "worker",
      enabled: false,
      declaration: {
        permissions: [{
          key: "worker.messaging",
          description: "Disabled synthetic Feature; no Worker messaging authority is implied.",
          children: [],
        }],
        sets: [],
      },
    },
  ],
};
function resolution(
  allowed: PermissionResolution["allowed"] = {},
  diagnostics: PermissionResolution["diagnostics"] = [],
): PermissionResolution {
  return { workspace: workspaceId, allowed, diagnostics };
}
function origin(setId: string, key: string): PermissionResolution["allowed"][string] {
  return [[
    { kind: "set", value: setId },
    { kind: "feature", value: "ticket" },
    ...(setId === "work-execution" ? [{ kind: "key" as const, value: "ticket.operations" }] : []),
    { kind: "key", value: key },
  ]];
}
function currentView(id: string): SubjectPermissionPolicyResponse {
  const policy: FixturePolicy = {
    change_id: `fixture-initial-${id}`,
    execution: assignment([set("conversation")]),
    delegation: assignment([set("work-execution")]),
    legacy_constraints: null,
    reference_encoding: "feature_sets_v2",
  };
  const view: SubjectPermissionPolicyResponse = {
    change_id: policy.change_id,
    policy,
    can_manage: id !== "permissions-reader",
    catalog: structuredClone(catalog),
    execution_resolution: resolution({ "ticket.read": origin("conversation", "ticket.read") }),
    delegation_resolution: resolution({
      "ticket.read": origin("work-execution", "ticket.read"),
      "ticket.comment": origin("work-execution", "ticket.comment"),
    }),
  };
  switch (id) {
    case "permissions-empty":
    case "permissions-removed":
      view.policy = null;
      view.change_id = id === "permissions-empty" ? null : "fixture-removal-permissions-removed";
      view.execution_resolution = resolution({}, [{ kind: "unset" }]);
      view.delegation_resolution = resolution({}, [{ kind: "unset" }]);
      break;
    case "permissions-explicit-empty":
      policy.execution = assignment();
      policy.delegation = assignment();
      view.execution_resolution = resolution();
      view.delegation_resolution = resolution();
      break;
    case "permissions-revoked":
      policy.execution.revoked = true;
      policy.delegation.revoked = true;
      view.execution_resolution = resolution({}, [{ kind: "revoked" }]);
      view.delegation_resolution = resolution({}, [{ kind: "revoked" }]);
      break;
    case "permissions-excluded":
      policy.execution = assignment(
        [set("work-execution")],
        [namespace("ticket", ["ticket.comment"])],
      );
      policy.delegation = assignment([set("work-execution")], [set("work-execution")]);
      view.execution_resolution = resolution(
        { "ticket.read": origin("work-execution", "ticket.read") },
        [{ kind: "excluded", value: "ticket.comment" }],
      );
      view.delegation_resolution = resolution({}, [
        { kind: "excluded", value: "ticket.read" },
        { kind: "excluded", value: "ticket.comment" },
      ]);
      break;
    case "permissions-legacy":
      // 旧参照を新しい包括grantへ読み替えず、限定された対象の証拠を残す。
      delete policy.reference_encoding;
      policy.execution = assignment([namespace("ticket", ["ticket.read"])]);
      policy.delegation = assignment([set("legacy-worker-notify")]);
      policy.legacy_constraints = {
        change_id: "fixture-retained-domain-constraints",
        execution: [{
          operation: "ticket.read",
          resource: { kind: "ticket", workspace_id: workspaceId, ticket_id: "T-802" },
        }],
        delegation: [structuredClone(fixtureLegacyPermission)],
      };
      view.execution_resolution = resolution({
        "ticket.read": [[
          { kind: "feature", value: "ticket" },
          { kind: "key", value: "ticket.read" },
        ]],
      });
      view.delegation_resolution = resolution({}, [
        { kind: "unknown_set", value: "legacy-worker-notify" },
      ]);
      break;
    case "permissions-longnames":
      view.catalog.namespaces.push({
        namespace: fixtureLongNames.namespace,
        feature: fixtureLongNames.feature,
        enabled: true,
        declaration: {
          permissions: [{
            key: fixtureLongNames.key,
            description: "Long identifiers must wrap without hiding the full value.",
            children: [],
          }],
          sets: [{
            set: fixtureLongNames.set,
            children: [namespace(fixtureLongNames.namespace, [fixtureLongNames.key])],
          }],
        },
      });
      policy.execution = assignment([set(fixtureLongNames.set)]);
      policy.delegation = assignment([namespace(fixtureLongNames.namespace, [fixtureLongNames.key])]);
      view.execution_resolution = resolution({
        [fixtureLongNames.key]: [[
          { kind: "set", value: fixtureLongNames.set },
          { kind: "feature", value: fixtureLongNames.feature },
          { kind: "key", value: fixtureLongNames.key },
        ]],
      });
      view.delegation_resolution = resolution({
        [fixtureLongNames.key]: [[
          { kind: "feature", value: fixtureLongNames.feature },
          { kind: "key", value: fixtureLongNames.key },
        ]],
      });
      break;
    case "permissions-diagnostics":
      policy.execution = assignment([
        set("missing-set"),
        namespace("missing-namespace", ["missing.operation"]),
        namespace("ticket", ["ticket.unknown"]),
        namespace("worker", ["worker.messaging"]),
      ]);
      policy.delegation = assignment();
      view.execution_resolution = resolution({}, [
        { kind: "unknown_set", value: "missing-set" },
        { kind: "unknown_namespace", value: "missing-namespace" },
        { kind: "unknown_key", value: "ticket.unknown" },
        { kind: "disabled_feature", value: "worker" },
      ]);
      view.delegation_resolution = resolution();
      break;
  }
  return view;
}
function json(value: unknown, status = 200): Response {
  return Response.json(value, {
    status,
    headers: { "x-yoi-evidence": "synthetic-http-not-real-backend" },
  });
}
function error(message: string, status: number): Response {
  return json({ error: "Synthetic fixture response", message }, status);
}

export function createSubjectPermissionsFixture() {
  const subjects = new Set<string>(permissionFixtureSubjects.map(([id]) => id));
  const requests: { path: string; method: string }[] = [];
  return async (request: Request): Promise<Response | null> => {
    const url = new URL(request.url);
    if (url.pathname === "/api/auth/whoami") {
      // 合成HTTP session projectionで実Webの画面入場判定を通す。
      // 実Backendのsessionやcookie/tokenは作成・迂回しない。
      return json({
        actor: {
          user_id: "synthetic-owner",
          account_id: "owner",
          handle: "fixture-owner",
          display_name: "Synthetic browser owner",
          auth_method: "browser_session",
        },
        session: { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" },
        authentication_reference: "synthetic-http-not-real-authentication",
      });
    }
    if (url.pathname === "/fixture/permissions/evidence") {
      if (request.method !== "GET") return error("GET required", 405);
      return json({ authority: "synthetic-http-not-real-backend", requests });
    }
    const match = url.pathname.match(
      /^\/api\/w\/([^/]+)\/subjektiv\/subjects\/([^/]+)\/permission-policy$/,
    );
    if (!match) return null;
    requests.push({ path: url.pathname, method: request.method });
    const [, workspace, id] = match.map(decodeURIComponent);
    if (workspace !== workspaceId || !subjects.has(id)) return error("Subject unavailable", 404);
    if (request.method !== "GET") return error("Read-only fixture; GET required", 405);
    if (id === "permissions-anonymous") return error("Authentication required", 401);
    if (id === "permissions-forbidden") return error("Subject policy access denied", 403);
    if (id === "permissions-unavailable") return error("Permission service unavailable", 503);
    return json(currentView(id));
  };
}
