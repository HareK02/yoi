/**
 * Browser UXだけの合成HTTP状態。実Backend、認証、認可、操作catalog、取消の実装ではない。
 * 実Backendの結合証拠は、このbrowser fixtureと分けて保持する。
 */
export const permissionFixtureSubjects = [
  ["permissions-empty", "New Subject — permissions not configured"],
  ["permissions-edit", "Release coordination"],
  ["permissions-legacy", "Subject with preserved legacy permissions"],
  ["permissions-reader", "Read-only Subject permissions"],
  ["permissions-unavailable", "Unavailable Subject permissions"],
] as const;

export type FixturePermission = {
  operation: string;
  resource: Record<string, string | boolean> & { kind: string };
};
export type FixturePolicy = {
  change_id: string;
  execution: FixturePermission[];
  delegation: FixturePermission[];
};
type FixtureUpdate = Omit<FixturePolicy, "change_id"> & {
  operation_id: string;
  expected_change_id: string | null;
};

const workspaceId = "memory-review";
const resource = { kind: "ticket", workspace_id: workspaceId, ticket_id: "T-802" };
export const fixtureLegacyPermission: FixturePermission = {
  operation: "worker.notify",
  resource: { kind: "worker", runtime_id: "legacy-runtime", worker_id: "legacy-recipient" },
};
// 小さな合成catalogに限定する。製品catalogの複製や第二正本ではない。
export const fixturePermissionOperations: {
  operation: string;
  label: string;
  description: string;
  resource_kinds: string[];
  availability: { kind: "available" } | { kind: "unavailable"; reason: string };
}[] = [
  {
    operation: "ticket.read",
    label: "Read ticket",
    description: "Read the selected ticket. Does not allow editing or closing it.",
    resource_kinds: ["ticket"],
    availability: { kind: "available" },
  },
  {
    operation: "ticket.comment",
    label: "Comment on ticket",
    description: "Append a comment to the selected ticket; no completion or assignment authority.",
    resource_kinds: ["ticket"],
    availability: { kind: "available" },
  },
  {
    operation: "worker.messaging",
    label: "Message a Worker (T-803)",
    description:
      "Operation use is separate from Backend directional target control grants and execution conditions.",
    resource_kinds: [],
    availability: {
      kind: "unavailable",
      reason: "T-803 is not implemented in this synthetic browser fixture.",
    },
  },
];

function json(value: unknown, status = 200): Response {
  return Response.json(value, {
    status,
    headers: { "x-yoi-evidence": "synthetic-http-not-real-backend" },
  });
}
function error(message: string, status: number): Response {
  return json({ error: "Synthetic fixture response", message }, status);
}
function isUpdate(value: unknown): value is FixtureUpdate {
  if (!value || typeof value !== "object") return false;
  const v = value as Record<string, unknown>;
  return typeof v.operation_id === "string" && v.operation_id.length > 0 &&
    (v.expected_change_id === null || typeof v.expected_change_id === "string") &&
    Array.isArray(v.execution) && Array.isArray(v.delegation) &&
    [...v.execution, ...v.delegation].every((p) =>
      p && typeof p.operation === "string" && p.resource && typeof p.resource.kind === "string"
    );
}

export function createSubjectPermissionsFixture() {
  const policies = new Map<string, FixturePolicy | null>();
  const receipts = new Map<string, { input: string; permissions: FixturePolicy }>();
  const requests: { subject_id: string; request: FixtureUpdate; outcome: string }[] = [];
  let issuedChangeCount = 0;
  for (const [id] of permissionFixtureSubjects) {
    policies.set(
      id,
      id === "permissions-empty" ? null : {
        change_id: `fixture-initial-${id}`,
        execution: [{ operation: "ticket.read", resource: { ...resource } }],
        delegation: id === "permissions-legacy" ? [structuredClone(fixtureLegacyPermission)] : [],
      },
    );
  }

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
      /^\/api\/w\/([^/]+)\/subjektiv\/subjects\/([^/]+)\/permissions$/,
    );
    if (!match) return null;
    const [, workspace, id] = match.map(decodeURIComponent);
    if (workspace !== workspaceId || !policies.has(id)) return error("Subject unavailable", 404);
    if (id === "permissions-unavailable") return error("Permission service unavailable", 503);
    const current = policies.get(id)!;
    if (request.method === "GET") {
      return json({
        permissions: current,
        can_manage: id !== "permissions-reader",
        operations: fixturePermissionOperations,
      });
    }
    if (request.method !== "POST") return error("POST required", 405);
    if (id === "permissions-reader") return error("Actor cannot manage permissions", 403);
    const payload: unknown = await request.json().catch(() => null);
    if (!isUpdate(payload)) return error("Invalid synthetic policy update", 400);
    const input = JSON.stringify(payload);
    const receiptKey = JSON.stringify([id, payload.operation_id]);
    const previous = receipts.get(receiptKey);
    if (previous) {
      requests.push({ subject_id: id, request: payload, outcome: "replay" });
      if (previous.input !== input) {
        return error("Operation identity reused with changed input", 409);
      }
      return json({ permissions: previous.permissions, interruptions: [] });
    }
    if (payload.expected_change_id !== (current?.change_id ?? null)) {
      requests.push({ subject_id: id, request: payload, outcome: "conflict" });
      return error("Policy changed in another browser; reload and review", 409);
    }
    // fixtureで使う組合せだけを拒否する。Backend検証の証拠にはしない。
    for (const p of [...payload.execution, ...payload.delegation]) {
      const legacy = JSON.stringify(p) === JSON.stringify(fixtureLegacyPermission) &&
        id === "permissions-legacy";
      const supported = fixturePermissionOperations.some((o) =>
        o.availability.kind === "available" && o.operation === p.operation &&
        o.resource_kinds.includes(p.resource.kind)
      );
      if (!legacy && !supported) {
        return error("Unknown operation or invalid resource combination", 400);
      }
      if (
        p.resource.kind === "ticket" &&
        (p.resource.workspace_id !== workspaceId || !p.resource.ticket_id)
      ) {
        return error("Invalid ticket target", 400);
      }
    }
    const saved: FixturePolicy = {
      change_id: `fixture-change-${++issuedChangeCount}`,
      execution: structuredClone(payload.execution),
      delegation: structuredClone(payload.delegation),
    };
    policies.set(id, saved);
    receipts.set(receiptKey, { input, permissions: structuredClone(saved) });
    requests.push({ subject_id: id, request: payload, outcome: "saved" });
    return json({ permissions: saved, interruptions: [] });
  };
}
