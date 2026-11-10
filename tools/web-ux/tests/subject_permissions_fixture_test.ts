import { assert, assertEquals, assertRejects } from "@std/assert";
import {
  parsePermissionResponse,
  PermissionHttpError,
  readPermissions,
} from "../../../web/workspace/src/lib/workspace/memory/permissions.ts";
import {
  createSubjectPermissionsFixture,
  fixtureLegacyPermission,
  fixtureLongNames,
  permissionFixtureSubjects,
} from "../browser-tests/subject_permissions_fixture.ts";

const origin = "http://127.0.0.1:12345";
const workspace = "memory-review";
const root = `${origin}/api/w/${workspace}/subjektiv/subjects/`;
function request(id: string, method = "GET"): Request {
  return new Request(`${root}${encodeURIComponent(id)}/permission-policy`, { method });
}
function transport(handle: ReturnType<typeof createSubjectPermissionsFixture>) {
  const requests: { path: string; method: string }[] = [];
  const fetchFixture: typeof fetch = async (input, init) => {
    const req = input instanceof Request
      ? new Request(input, init)
      : new Request(new URL(String(input), origin), init);
    requests.push({ path: new URL(req.url).pathname, method: req.method });
    const response = await handle(req);
    assert(response, `Unexpected fixture request: ${req.method} ${req.url}`);
    return response;
  };
  return { fetchFixture, requests };
}
const errorScenarios = new Map([
  ["permissions-anonymous", 401],
  ["permissions-forbidden", 403],
  ["permissions-unavailable", 503],
]);
const readableSubjects = permissionFixtureSubjects.filter(([id]) => !errorScenarios.has(id));

Deno.test("synthetic browser admission projection is explicitly labeled, with no real credentials", async () => {
  const handle = createSubjectPermissionsFixture();
  const response = await handle(new Request(`${origin}/api/auth/whoami`));
  assert(response);
  assertEquals(response.headers.get("x-yoi-evidence"), "synthetic-http-not-real-backend");
  assertEquals(response.headers.get("set-cookie"), null);
  const view = await response.json();
  assertEquals(view.authentication_reference, "synthetic-http-not-real-authentication");
  assertEquals(view.session.kind, "authenticated");
  assertEquals(view.actor.auth_method, "browser_session");
});

Deno.test("dedicated policy GET fixtures satisfy the production current-policy parser", async () => {
  const handle = createSubjectPermissionsFixture();
  for (const [id] of readableSubjects) {
    const response = await handle(request(id));
    assert(response);
    assertEquals(response.status, 200, id);
    assertEquals(response.headers.get("x-yoi-evidence"), "synthetic-http-not-real-backend");
    const body = await response.json();
    assertEquals(Object.keys(body).sort(), [
      "can_manage",
      "catalog",
      "change_id",
      "delegation_resolution",
      "execution_resolution",
      "policy",
    ]);
    const view = parsePermissionResponse(body, workspace);
    assertEquals(view.can_manage, id !== "permissions-reader", id);
    assertEquals(view.catalog.workspace, workspace);
    assertEquals(view.execution_resolution.workspace, workspace);
    assertEquals(view.delegation_resolution.workspace, workspace);
    assertEquals(view.readOnly, id === "permissions-reader" || id === "permissions-legacy", id);
    if (view.policy) {
      assertEquals(view.policy.change_id, view.change_id);
      assertEquals(Object.hasOwn(body.policy, "execution_resolution"), false);
      assertEquals(Object.hasOwn(body.policy, "delegation_resolution"), false);
    }
  }
});

Deno.test("every Subject permissions read uses only dedicated GET, never config/adoption", async () => {
  const handle = createSubjectPermissionsFixture();
  const { fetchFixture, requests } = transport(handle);
  for (const [id] of readableSubjects) {
    await readPermissions(fetchFixture, workspace, id);
  }
  for (const [id, status] of errorScenarios) {
    const failure = await assertRejects(
      () => readPermissions(fetchFixture, workspace, id),
      PermissionHttpError,
    );
    assertEquals(failure.status, status);
  }
  assertEquals(requests, [
    ...readableSubjects.map(([id]) => id),
    ...errorScenarios.keys(),
  ].map((id) => ({
    path: `/api/w/${workspace}/subjektiv/subjects/${id}/permission-policy`,
    method: "GET",
  })));
  assertEquals(requests.some(({ path }) => path.includes("/config/adoption")), false);
  assertEquals(requests.some(({ path }) => path.includes("/config/")), false);
  const evidence = await handle(new Request(`${origin}/fixture/permissions/evidence`));
  assert(evidence);
  assertEquals((await evidence.json()).requests, requests);
  // configはこのfixtureの所有外とし、権限GETのfallbackを用意しない。
  for (const method of ["GET", "POST", "PUT"]) {
    assertEquals(
      await handle(new Request(`${origin}/api/w/${workspace}/config/adoption`, { method })),
      null,
    );
  }
});

Deno.test("unset and removal tombstone remain distinct from explicitly empty assignments", async () => {
  const { fetchFixture } = transport(createSubjectPermissionsFixture());
  const unset = await readPermissions(fetchFixture, workspace, "permissions-empty");
  const removed = await readPermissions(fetchFixture, workspace, "permissions-removed");
  const empty = await readPermissions(fetchFixture, workspace, "permissions-explicit-empty");
  assertEquals(unset.policy, null);
  assertEquals(unset.change_id, null);
  assertEquals(removed.policy, null);
  assert(removed.change_id);
  for (const view of [unset, removed]) {
    for (const lane of ["execution_resolution", "delegation_resolution"] as const) {
      assertEquals(view[lane].allowed, {});
      assertEquals(view[lane].diagnostics, [{ kind: "unset" }]);
    }
  }
  assert(empty.policy);
  assertEquals(empty.policy.change_id, empty.change_id);
  for (const lane of ["execution", "delegation"] as const) {
    assertEquals(empty.policy[lane], { grants: [], exclusions: [], revoked: false });
  }
  assertEquals(empty.execution_resolution.allowed, {});
  assertEquals(empty.delegation_resolution.allowed, {});
  assertEquals(empty.execution_resolution.diagnostics, []);
  assertEquals(empty.delegation_resolution.diagnostics, []);
});

Deno.test("normal current policy keeps execution and delegation assignments and origins separate", async () => {
  const { fetchFixture } = transport(createSubjectPermissionsFixture());
  const view = await readPermissions(fetchFixture, workspace, "permissions-edit");
  assert(view.policy);
  assertEquals(view.policy.execution.grants, [{ kind: "set", value: "conversation" }]);
  assertEquals(view.policy.delegation.grants, [{ kind: "set", value: "work-execution" }]);
  assertEquals(Object.keys(view.execution_resolution.allowed), ["ticket.read"]);
  assertEquals(Object.keys(view.delegation_resolution.allowed).sort(), [
    "ticket.comment",
    "ticket.read",
  ]);
  assertEquals(view.execution_resolution.allowed["ticket.read"], [[
    { kind: "set", value: "conversation" },
    { kind: "feature", value: "ticket" },
    { kind: "key", value: "ticket.read" },
  ]]);
  assertEquals(view.delegation_resolution.allowed["ticket.comment"], [[
    { kind: "set", value: "work-execution" },
    { kind: "feature", value: "ticket" },
    { kind: "key", value: "ticket.operations" },
    { kind: "key", value: "ticket.comment" },
  ]]);
});

Deno.test("revoked references resolve to nothing and exclusions remove only declared keys", async () => {
  const { fetchFixture } = transport(createSubjectPermissionsFixture());
  const revoked = await readPermissions(fetchFixture, workspace, "permissions-revoked");
  assert(revoked.policy);
  for (const lane of ["execution", "delegation"] as const) {
    assertEquals(revoked.policy[lane].revoked, true);
    assert(revoked.policy[lane].grants.length > 0);
  }
  for (const lane of ["execution_resolution", "delegation_resolution"] as const) {
    assertEquals(revoked[lane].allowed, {});
    assertEquals(revoked[lane].diagnostics, [{ kind: "revoked" }]);
  }
  const excluded = await readPermissions(fetchFixture, workspace, "permissions-excluded");
  assert(excluded.policy);
  assertEquals(excluded.policy.execution.exclusions, [{
    kind: "namespace",
    value: { namespace: "ticket", keys: ["ticket.comment"] },
  }]);
  assertEquals(Object.keys(excluded.execution_resolution.allowed), ["ticket.read"]);
  assertEquals(excluded.execution_resolution.diagnostics, [{
    kind: "excluded",
    value: "ticket.comment",
  }]);
  assertEquals(excluded.policy.delegation.exclusions, [{ kind: "set", value: "work-execution" }]);
  assertEquals(excluded.delegation_resolution.allowed, {});
  assertEquals(excluded.delegation_resolution.diagnostics, [
    { kind: "excluded", value: "ticket.read" },
    { kind: "excluded", value: "ticket.comment" },
  ]);
});

Deno.test("retained domain restrictions do not turn old Worker identity into a current grant", async () => {
  const { fetchFixture } = transport(createSubjectPermissionsFixture());
  const view = await readPermissions(fetchFixture, workspace, "permissions-legacy");
  assert(view.policy);
  assertEquals(view.readOnly, true);
  assertEquals(view.policy.legacy_constraints?.execution, [{
    operation: "ticket.read",
    resource: { kind: "ticket", workspace_id: workspace, ticket_id: "T-802" },
  }]);
  assertEquals(view.policy.legacy_constraints?.delegation, [fixtureLegacyPermission]);
  assertEquals(view.delegation_resolution.allowed, {});
  assertEquals(view.delegation_resolution.diagnostics, [{
    kind: "unknown_set",
    value: "legacy-worker-notify",
  }]);
  assertEquals(view.catalog.namespaces.find((entry) => entry.namespace === "worker")?.enabled, false);
});

Deno.test("long identifiers survive dedicated policy, catalog and origin parsing without truncation", async () => {
  const { fetchFixture } = transport(createSubjectPermissionsFixture());
  const view = await readPermissions(fetchFixture, workspace, "permissions-longnames");
  assert(view.policy);
  assertEquals(view.policy.execution.grants, [{ kind: "set", value: fixtureLongNames.set }]);
  assertEquals(view.policy.delegation.grants, [{
    kind: "namespace",
    value: { namespace: fixtureLongNames.namespace, keys: [fixtureLongNames.key] },
  }]);
  const declaration = view.catalog.namespaces.find((entry) =>
    entry.namespace === fixtureLongNames.namespace
  );
  assert(declaration);
  assertEquals(declaration.feature, fixtureLongNames.feature);
  assertEquals(declaration.declaration.sets[0].set, fixtureLongNames.set);
  assertEquals(declaration.declaration.permissions[0].key, fixtureLongNames.key);
  assertEquals(Object.keys(view.execution_resolution.allowed), [fixtureLongNames.key]);
  assertEquals(Object.keys(view.delegation_resolution.allowed), [fixtureLongNames.key]);
  assertEquals(view.execution_resolution.allowed[fixtureLongNames.key], [[
    { kind: "set", value: fixtureLongNames.set },
    { kind: "feature", value: fixtureLongNames.feature },
    { kind: "key", value: fixtureLongNames.key },
  ]]);
});

Deno.test("unknown references and disabled Features produce diagnostics, not operation grants", async () => {
  const { fetchFixture } = transport(createSubjectPermissionsFixture());
  const view = await readPermissions(fetchFixture, workspace, "permissions-diagnostics");
  assert(view.policy);
  assertEquals(view.policy.execution.grants.length, 4);
  assertEquals(view.execution_resolution.allowed, {});
  assertEquals(view.execution_resolution.diagnostics, [
    { kind: "unknown_set", value: "missing-set" },
    { kind: "unknown_namespace", value: "missing-namespace" },
    { kind: "unknown_key", value: "ticket.unknown" },
    { kind: "disabled_feature", value: "worker" },
  ]);
  assertEquals(view.policy.delegation.grants, []);
  assertEquals(view.delegation_resolution.allowed, {});
});

Deno.test("policy GET fixture distinguishes auth errors, unavailable and unknown targets", async () => {
  const handle = createSubjectPermissionsFixture();
  for (const [id, status] of [...errorScenarios, ["missing", 404] as const]) {
    const response = await handle(request(id));
    assert(response);
    assertEquals(response.status, status, id);
    const body = await response.json();
    assertEquals(typeof body.message, "string");
    assertEquals(Object.hasOwn(body, "policy"), false);
  }
  const wrongWorkspace = await handle(new Request(
    `${root.replace(workspace, "other")}permissions-edit/permission-policy`,
  ));
  assertEquals(wrongWorkspace?.status, 404);
  const reader = await handle(request("permissions-reader"));
  assert(reader);
  assertEquals((await reader.json()).can_manage, false);
  for (const method of ["POST", "PUT", "DELETE"]) {
    assertEquals((await handle(request("permissions-edit", method)))?.status, 405);
  }
  assertEquals(await handle(new Request(`${root}permissions-edit/permissions`)), null);
  assertEquals(await handle(new Request(`${origin}/not-owned`)), null);
});
