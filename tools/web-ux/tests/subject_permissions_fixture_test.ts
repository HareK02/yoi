import { assert, assertEquals } from "@std/assert";
import {
  createSubjectPermissionsFixture,
  fixtureLegacyPermission,
  type FixturePolicy,
} from "../browser-tests/subject_permissions_fixture.ts";

const root = "http://127.0.0.1:12345/api/w/memory-review/subjektiv/subjects/";
const target = { kind: "ticket", workspace_id: "memory-review", ticket_id: "T-802" };
const direct = { operation: "ticket.read", resource: target };
const delegated = { operation: "ticket.comment", resource: target };
function request(id: string, body?: unknown): Request {
  return new Request(
    `${root}${id}/permissions`,
    body === undefined ? {} : {
      method: "POST",
      body: JSON.stringify(body),
      headers: { "content-type": "application/json" },
    },
  );
}

Deno.test("synthetic browser admission projection is explicitly labeled, with no real credentials", async () => {
  const handle = createSubjectPermissionsFixture();
  const response = await handle(new Request("http://127.0.0.1:12345/api/auth/whoami"));
  assert(response);
  assertEquals(response.headers.get("x-yoi-evidence"), "synthetic-http-not-real-backend");
  assertEquals(response.headers.get("set-cookie"), null);
  const view = await response.json();
  assertEquals(view.authentication_reference, "synthetic-http-not-real-authentication");
  assertEquals(view.session.kind, "authenticated");
  assertEquals(view.actor.auth_method, "browser_session");
});

Deno.test("synthetic UX fixture: unset, separate lanes, exact replay and two-editor CAS", async () => {
  const handle = createSubjectPermissionsFixture();
  const initial = await handle(request("permissions-empty"));
  assert(initial);
  assertEquals(initial.headers.get("x-yoi-evidence"), "synthetic-http-not-real-backend");
  assertEquals((await initial.json()).permissions, null);
  const update = {
    operation_id: "browser-save-1",
    expected_change_id: null,
    execution: [direct],
    delegation: [delegated],
  };
  const save = await handle(request("permissions-empty", update));
  assert(save);
  assertEquals(save.status, 200);
  const saved = await save.json();
  assertEquals(saved.permissions.execution, [direct]);
  assertEquals(saved.permissions.delegation, [delegated]);
  const replay = await handle(request("permissions-empty", update));
  assert(replay);
  assertEquals(await replay.json(), saved);
  const collision = await handle(request("permissions-empty", { ...update, execution: [] }));
  assertEquals(collision?.status, 409);
  const stale = await handle(
    request("permissions-empty", { ...update, operation_id: "second-tab" }),
  );
  assertEquals(stale?.status, 409);
  const readback = await handle(request("permissions-empty"));
  assertEquals((await readback!.json()).permissions, saved.permissions);
});

Deno.test("synthetic UX fixture: read-only, unavailable, wrong workspace and unknown targets", async () => {
  const handle = createSubjectPermissionsFixture();
  const view = await handle(request("permissions-reader"));
  assertEquals((await view!.json()).can_manage, false);
  assertEquals((await handle(request("permissions-reader", {})))?.status, 403);
  assertEquals((await handle(request("permissions-unavailable")))?.status, 503);
  assertEquals((await handle(request("missing")))?.status, 404);
  assertEquals(
    (await handle(
      new Request(`${root.replace("memory-review", "other")}permissions-edit/permissions`),
    ))?.status,
    404,
  );
  assertEquals(await handle(new Request("http://127.0.0.1:12345/not-owned")), null);
});

Deno.test("synthetic UX fixture preserves old Worker target without making T-803 usable", async () => {
  const handle = createSubjectPermissionsFixture();
  const view = await handle(request("permissions-legacy"));
  const initial = await view!.json();
  assertEquals(initial.permissions.delegation, [fixtureLegacyPermission]);
  const workerOperation = initial.operations.find((o: { operation: string }) =>
    o.operation === "worker.messaging"
  );
  assertEquals(workerOperation.availability.kind, "unavailable");
  assertEquals(workerOperation.resource_kinds, []);
  assertEquals(Object.hasOwn(workerOperation, "available"), false);
  assertEquals(Object.hasOwn(workerOperation, "unavailable_reason"), false);
  assertEquals(
    initial.operations.find((o: { operation: string }) => o.operation === "ticket.read")
      .availability,
    { kind: "available" },
  );
  const update = {
    operation_id: "preserve-legacy",
    expected_change_id: initial.permissions.change_id,
    execution: [direct, delegated],
    delegation: initial.permissions.delegation,
  };
  const saved = await handle(request("permissions-legacy", update));
  assertEquals(saved?.status, 200);
  const value = await saved!.json();
  assertEquals(value.permissions.delegation, [fixtureLegacyPermission]);
  const policy = value.permissions as FixturePolicy;
  for (
    const invalid of [
      { operation: "future.all", resource: target },
      { operation: "ticket.read", resource: { kind: "workspace", workspace_id: "memory-review" } },
      {
        operation: "worker.messaging",
        resource: { kind: "workspace", workspace_id: "memory-review" },
      },
    ]
  ) {
    const rejected = await handle(request("permissions-legacy", {
      ...update,
      operation_id: `reject-${invalid.operation}`,
      expected_change_id: policy.change_id,
      execution: [invalid],
    }));
    assertEquals(rejected?.status, 400);
  }
  assertEquals((await (await handle(request("permissions-legacy")))!.json()).permissions, policy);
});
