import {
  attemptKey,
  makeResource,
  parseView,
  permissionKey,
  policyDiff,
  readAttempt,
  readPermissions,
  writePermissions,
} from "./permissions.ts";
import type { ScopedPermission } from "./permissions.ts";
declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};
function assert(condition: unknown): asserts condition {
  if (!condition) throw new Error("Assertion failed");
}
const p: ScopedPermission = {
  operation: "filesystem.read",
  resource: {
    kind: "workdir",
    working_directory_id: "wd",
    path: "src",
    recursive: true,
  },
};
Deno.test("permission differences preserve exact legacy resource and separate lanes", () => {
  const legacy: ScopedPermission = {
    operation: "worker.notify",
    resource: {
      kind: "worker",
      runtime_id: "runtime",
      worker_id: "only-worker",
    },
  };
  const direct = [legacy, p];
  assert(policyDiff([legacy], direct).added.length === 1);
  assert(policyDiff([legacy], direct).removed.length === 0);
  assert(
    permissionKey(p) ===
      permissionKey({
        operation: p.operation,
        resource: {
          recursive: true,
          path: "src",
          working_directory_id: "wd",
          kind: "workdir",
        },
      }),
  );
  assert(policyDiff([legacy], []).removed[0] === legacy);
});
Deno.test("typed resource input binds workspace, preserves narrow paths and rejects missing targets", () => {
  assert(
    JSON.stringify(
      makeResource("workdir", "w", "s", {
        working_directory_id: "wd",
        path: "src",
      }, false),
    ) ===
      JSON.stringify({
        kind: "workdir",
        working_directory_id: "wd",
        path: "src",
        recursive: false,
      }),
  );
  assert(
    JSON.stringify(makeResource("subject", "w", "s", {}, false)) ===
      JSON.stringify({ kind: "subject", workspace_id: "w", subject_id: "s" }),
  );
  let rejected = false;
  try {
    makeResource("ticket", "w", "s", {}, false);
  } catch {
    rejected = true;
  }
  assert(rejected);
});
Deno.test("catalog parsing refuses unknown resource types and malformed management permission", () => {
  for (
    const value of [
      { permissions: null, can_manage: "yes", operations: [] },
      {
        permissions: null,
        can_manage: true,
        operations: [{
          operation: "unknown",
          label: "Unknown",
          description: "",
          resource_kinds: ["everything"],
          availability: { kind: "available" },
        }],
      },
    ]
  ) {
    let rejected = false;
    try {
      parseView(value);
    } catch {
      rejected = true;
    }
    assert(rejected);
  }
});
Deno.test("save retry keeps exact complete input scoped to Workspace and Subject", async () => {
  const request = {
    operation_id: "stable-operation",
    expected_change_id: "old",
    execution: [p],
    delegation: [],
  };
  const memory = new Map([[attemptKey("w", "s"), JSON.stringify(request)]]);
  const storage = { getItem: (key: string) => memory.get(key) ?? null };
  assert(
    JSON.stringify(readAttempt(storage, "w", "s")) === JSON.stringify(request),
  );
  assert(readAttempt(storage, "other", "s") === null);
  assert(readAttempt(storage, "w", "other") === null);
  const calls: { url: string; body: string }[] = [];
  const fake = ((input: string, init?: RequestInit) => {
    calls.push({ url: input, body: init?.body as string });
    return Promise.resolve(
      Response.json({
        permissions: { change_id: "new", execution: [p], delegation: [] },
        interruptions: [],
      }),
    );
  }) as typeof fetch;
  await writePermissions(fake, "w", "s", request);
  await writePermissions(fake, "w", "s", readAttempt(storage, "w", "s")!);
  assert(calls[0].url === "/api/w/w/subjektiv/subjects/s/permissions");
  assert(calls[0].body === calls[1].body);
});
Deno.test("read permission projection uses the existing scoped endpoint and no fallback grant", async () => {
  let path = "";
  const fake = ((input: string) => {
    path = input;
    return Promise.resolve(
      Response.json({ permissions: null, can_manage: false, operations: [] }),
    );
  }) as typeof fetch;
  const view = await readPermissions(fake, "space", "subject");
  assert(path === "/api/w/space/subjektiv/subjects/subject/permissions");
  assert(
    view.permissions === null && !view.can_manage &&
      view.operations.length === 0,
  );
});

Deno.test("Worker operation editor binds operation use without a target and preserves installed Plugin scopes", () => {
  const resource = makeResource("worker_operation", "w", "s", {}, false);
  assert(
    JSON.stringify(resource) ===
      JSON.stringify({ kind: "worker_operation", workspace_id: "w" }),
  );
  const view = parseView({
    permissions: {
      change_id: "saved",
      execution: [{ operation: "worker.messaging", resource }],
      delegation: [],
    },
    can_manage: true,
    operations: [{
      operation: "worker.messaging",
      label: "Message Workers",
      description: "Directional target grants remain separate",
      resource_kinds: ["worker_operation"],
      availability: { kind: "available" },
    }],
  });
  assert(
    view.operations[0].available &&
      view.permissions?.execution[0].resource.kind === "worker_operation",
  );
  const installed = makeResource("plugin_installation", "w", "s", {
    installation_id: "installed",
  }, false);
  assert(
    installed.kind === "plugin_installation" &&
      installed.workspace_id === "w" &&
      installed.installation_id === "installed",
  );
});
