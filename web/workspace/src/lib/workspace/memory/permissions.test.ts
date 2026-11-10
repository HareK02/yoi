import {
  deepStrictEqual as assertEquals,
  rejects as assertRejects,
  throws as assertThrows,
} from "node:assert/strict";
declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};
import {
  parseAssignment,
  parseCatalog,
  parsePermissionMutationResponse,
  parsePermissionResponse,
  parsePolicy,
  parseResolution,
  PermissionHttpError,
  PermissionMutationUnknownError,
  readPermissions,
  referenceLabel,
  savePermissions,
} from "./permissions.ts";
import type {
  Policy,
  SubjectPermissionPolicyMutation,
  SubjectPermissionPolicyMutationResponse,
  SubjectPermissionPolicyResponse,
} from "./permissions.ts";
import { permissionResolution } from "../../../../test/permissions-fixture.ts";
import catalogFixture from "../../../../../../crates/feature-permissions/tests/fixtures/catalog.json" with {
  type: "json",
};
import assignmentFixture from "../../../../../../crates/feature-permissions/tests/fixtures/assignment.json" with {
  type: "json",
};

function currentPolicy(): Policy {
  return {
    change_id: "policy:observed",
    execution: {
      grants: [{ kind: "set", value: "ticket" }, {
        kind: "namespace",
        value: { namespace: "ticket", keys: ["ticket.hoge.fuga.finish"] },
      }],
      exclusions: [{
        kind: "namespace",
        value: { namespace: "ticket", keys: ["ticket.delete"] },
      }],
      revoked: false,
    },
    delegation: {
      grants: [{ kind: "set", value: "work-execution" }],
      exclusions: [{ kind: "set", value: "excluded-set" }],
      revoked: true,
    },
    legacy_constraints: {
      change_id: "legacy:observed",
      execution: [{
        operation: "ticket.read",
        resource: { kind: "ticket", workspace_id: "w", ticket_id: "T-837" },
      }],
      delegation: [],
    },
    reference_encoding: "feature_sets_v2",
  };
}
function mutationResponse(): SubjectPermissionPolicyMutationResponse {
  return { change_id: "policy:observed", policy: currentPolicy() };
}
function policyResponse(workspace = "w/one"): SubjectPermissionPolicyResponse {
  return {
    ...mutationResponse(),
    can_manage: true,
    catalog: parseCatalog({ ...catalogFixture, workspace }),
    execution_resolution: permissionResolution(workspace),
    delegation_resolution: permissionResolution(workspace, true),
  };
}
function replaceMutation(): SubjectPermissionPolicyMutation {
  const policy = currentPolicy();
  return {
    operation_id: "replace:one",
    expected_change_id: policyResponse().change_id,
    action: {
      kind: "replace",
      execution: policy.execution,
      delegation: policy.delegation,
    },
  };
}
const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status });
const subjectPath = "/api/w/w%2Fone/subjektiv/subjects/s%2Fone/permission-policy";
function mockNetwork(
  respond: (init: RequestInit | undefined) => Response | Promise<Response>,
) {
  const calls: { path: string; method: string; body: unknown }[] = [];
  const fetcher = (async (input: unknown, init?: RequestInit) => {
    // Any config/adoption/catalog/validate call is a test failure, not a mock reply.
    assertEquals(String(input), subjectPath);
    calls.push({
      path: String(input),
      method: init?.method ?? "GET",
      body: init?.body ? JSON.parse(String(init.body)) : null,
    });
    return await respond(init);
  }) as typeof fetch;
  return { calls, fetcher };
}

Deno.test("Feature declaration and Set/Namespace parsers preserve shared Rust multilevel children without prefix inference", () => {
  assertEquals(parseCatalog(catalogFixture), catalogFixture);
  assertEquals(parseAssignment(assignmentFixture), assignmentFixture);
  assertEquals(
    referenceLabel({ kind: "set", value: "ticket" }),
    "Set · ticket",
  );
  assertEquals(
    referenceLabel({
      kind: "namespace",
      value: { namespace: "ticket", keys: ["ticket.hoge.fuga.finish"] },
    }),
    "Namespace · ticket · ticket.hoge.fuga.finish",
  );
});
Deno.test("permission assignments reject per-key payloads, legacy resources and inferred kinds", () => {
  const assignment = currentPolicy().execution;
  for (
    const ref of [
      { kind: "ticket", value: "ticket" },
      { kind: "set", value: "ticket", data: {} },
      {
        kind: "namespace",
        value: {
          namespace: "ticket",
          keys: [{ key: "ticket.read", data: {} }],
        },
      },
      {
        kind: "namespace",
        value: { namespace: "ticket", keys: ["ticket.read"], schema: {} },
      },
      {
        operation: "ticket.read",
        resource: { kind: "ticket", ticket_id: "T-1" },
      },
    ]
  ) assertThrows(() => parseAssignment({ ...assignment, grants: [ref] }));
});
Deno.test("current policy preserves exclusions, revocation, encoding and read-only legacy evidence", () => {
  const policy = currentPolicy();
  assertEquals(parsePolicy(policy), policy);
  const response = policyResponse();
  assertEquals(parsePermissionResponse(response, "w/one"), {
    ...response,
    readOnly: false,
  });
  assertEquals(
    parsePermissionResponse({ ...response, can_manage: false }, "w/one").readOnly,
    true,
  );
  for (const encoding of [undefined, "legacy_subject_v1"] as const) {
    const { reference_encoding: _encoding, ...lanes } = policy;
    const legacy = encoding ? { ...lanes, reference_encoding: encoding } : lanes;
    const view = parsePermissionResponse({ ...response, policy: legacy }, "w/one");
    assertEquals(view.policy, legacy);
    assertEquals(view.readOnly, true);
  }
});
Deno.test("dedicated response requires identity and coherent projections without configuration fallbacks", () => {
  const response = policyResponse();
  const policy = currentPolicy();
  for (const value of [
    {},
    { policy },
    { ...response, can_manage: "true" },
    { ...response, source: {} },
    { ...response, policy: { ...policy, subject_id: "s" } },
    { ...response, policy: { ...policy, reference_encoding: "future" } },
    { ...response, policy: { ...policy, execution: [] } },
    { ...response, policy: { ...policy, legacy_constraints: {} } },
    { ...response, policy: { ...policy, change_id: null } },
    { ...response, change_id: null },
    { ...response, change_id: "different-policy" },
    { ...response, change_id: 123 },
    { ...response, catalog: { ...response.catalog, workspace: "other" } },
    { ...response, execution_resolution: permissionResolution("other") },
    { ...response, delegation_resolution: permissionResolution("other") },
    { ...response, execution_resolution: { workspace: "w/one", allowed: {}, diagnostics: null } },
  ]) assertThrows(() => parsePermissionResponse(value, "w/one"));
  for (const field of Object.keys(response)) {
    const incomplete: Record<string, unknown> = { ...response };
    delete incomplete[field];
    assertThrows(() => parsePermissionResponse(incomplete, "w/one"));
  }
});
Deno.test("mutation responses require independent state identity and agree with any present policy", () => {
  const response = mutationResponse();
  assertEquals(parsePermissionMutationResponse(response), response);
  for (const change_id of [null, "policy:removed"]) {
    assertEquals(parsePermissionMutationResponse({ change_id, policy: null }), {
      change_id,
      policy: null,
    });
  }
  for (const value of [
    { policy: response.policy },
    { change_id: response.change_id },
    { ...response, change_id: undefined },
    { ...response, change_id: null },
    { ...response, change_id: "mismatched" },
    { change_id: 123, policy: null },
    { ...response, can_manage: true },
  ]) assertThrows(() => parsePermissionMutationResponse(value));
});
Deno.test("resolution parser preserves server origins but does not infer graph edges", () => {
  const resolution = permissionResolution();
  assertEquals(parseResolution(resolution, "w"), resolution);
  assertThrows(() => parseResolution(resolution, "other"));
  assertThrows(() =>
    parseResolution({
      ...resolution,
      allowed: { "ticket.hoge": [[{ kind: "prefix", value: "ticket" }]] },
    }, "w")
  );
});
Deno.test("permission read calls only the dedicated encoded Subject path and honors can_manage", async () => {
  for (const can_manage of [true, false]) {
    const response = { ...policyResponse(), can_manage };
    const { calls, fetcher } = mockNetwork(() => json(response));
    assertEquals(await readPermissions(fetcher, "w/one", "s/one"), {
      ...response,
      readOnly: !can_manage,
    });
    assertEquals(calls, [{ path: subjectPath, method: "GET", body: null }]);
  }
});
Deno.test("refresh replaces policy, identity, catalog and both server resolutions together", async () => {
  const first = policyResponse();
  const next = {
    ...policyResponse(),
    change_id: "next",
    policy: { ...currentPolicy(), change_id: "next", execution: currentPolicy().delegation },
    can_manage: false,
    catalog: { workspace: "w/one", namespaces: [] },
    execution_resolution: permissionResolution("w/one", true),
  };
  const replies = [first, next];
  const { fetcher, calls } = mockNetwork(() => json(replies.shift()));
  const before = await readPermissions(fetcher, "w/one", "s/one");
  const after = await readPermissions(fetcher, "w/one", "s/one");
  assertEquals(before, { ...first, readOnly: false });
  assertEquals(after, { ...next, readOnly: true });
  assertEquals(calls.map((call) => call.method), ["GET", "GET"]);
});
Deno.test("GET preserves never-set and removal tombstone identities without inventing defaults or resolutions", async () => {
  for (const change_id of [null, "policy:removed"]) {
    const response = {
      ...policyResponse(),
      change_id,
      policy: null,
      execution_resolution: {
        workspace: "w/one", allowed: {}, diagnostics: [{ kind: "unset" }],
      },
      delegation_resolution: {
        workspace: "w/one", allowed: {}, diagnostics: [{ kind: "unset" }],
      },
    };
    const { fetcher, calls } = mockNetwork(() => json(response));
    assertEquals(await readPermissions(fetcher, "w/one", "s/one"), {
      ...response,
      readOnly: false,
    });
    assertEquals(calls, [{ path: subjectPath, method: "GET", body: null }]);
    const { change_id: _missing, ...incomplete } = response;
    assertThrows(() => parsePermissionResponse(incomplete, "w/one"));
  }
});
Deno.test("read failures never become an empty assignment or trigger a config fallback", async () => {
  for (const status of [401, 403, 404, 409, 500]) {
    const { fetcher, calls } = mockNetwork(() => json({}, status));
    await assertRejects(
      () => readPermissions(fetcher, "w/one", "s/one"),
      PermissionHttpError,
    );
    assertEquals(calls.length, 1);
  }
  for (const body of [
    {},
    { policy: null },
    { policy: {}, can_manage: true },
    policyResponse("other-workspace"),
    { ...policyResponse(), execution_resolution: permissionResolution("other") },
    { ...policyResponse(), delegation_resolution: permissionResolution("other") },
  ]) {
    const { fetcher, calls } = mockNetwork(() => json(body));
    await assertRejects(() => readPermissions(fetcher, "w/one", "s/one"));
    assertEquals(calls.length, 1);
  }
});
Deno.test("replace PUT contains Subject CAS and both exact assignments, never config source or policy metadata", async () => {
  const mutation = replaceMutation();
  const response = mutationResponse();
  const { fetcher, calls } = mockNetwork((init) => {
    assertEquals(init?.headers, { "content-type": "application/json" });
    return json(response);
  });
  assertEquals(await savePermissions(fetcher, "w/one", "s/one", mutation), response);
  assertEquals(calls, [{ path: subjectPath, method: "PUT", body: mutation }]);
  assertEquals(Object.keys(calls[0].body as object), [
    "operation_id", "expected_change_id", "action",
  ]);
});
Deno.test("remove returns a tombstone and recreation uses the observed response identity instead of null CAS", async () => {
  const removed = { change_id: "policy:removed", policy: null };
  const restored = mutationResponse();
  const replies = [
    json({ ...policyResponse(), ...removed }),
    json(restored),
    json(removed),
  ];
  const { fetcher, calls } = mockNetwork(() => {
    const reply = replies.shift();
    if (!reply) throw new Error("unexpected request");
    return reply;
  });
  const observed = await readPermissions(fetcher, "w/one", "s/one");
  assertEquals(observed.policy, null);
  const recreate = { ...replaceMutation(), expected_change_id: observed.change_id };
  const saved = await savePermissions(fetcher, "w/one", "s/one", recreate);
  assertEquals(saved, restored);
  const remove: SubjectPermissionPolicyMutation = {
    operation_id: "remove:one",
    expected_change_id: saved.change_id,
    action: { kind: "remove" },
  };
  assertEquals(await savePermissions(fetcher, "w/one", "s/one", remove), removed);
  assertEquals(calls, [
    { path: subjectPath, method: "GET", body: null },
    { path: subjectPath, method: "PUT", body: recreate },
    { path: subjectPath, method: "PUT", body: remove },
  ]);
  assertEquals(recreate.expected_change_id, "policy:removed");
});
Deno.test("explicit create and remove send observed null CAS only for a never-set state", async () => {
  for (const action of [replaceMutation().action, { kind: "remove" } as const]) {
    const response = action.kind === "replace"
      ? mutationResponse()
      : { change_id: "policy:removed", policy: null };
    const mutation = { operation_id: "explicit", expected_change_id: null, action };
    const { fetcher, calls } = mockNetwork(() => json(response));
    assertEquals(await savePermissions(fetcher, "w/one", "s/one", mutation), response);
    assertEquals(calls, [{ path: subjectPath, method: "PUT", body: mutation }]);
  }
});
Deno.test("invalid saves reject config/adoption or stored-policy inputs before any request", async () => {
  const mutation = replaceMutation();
  for (const invalid of [
    { ...mutation, operation_id: "" },
    { ...mutation, expected_change_id: undefined },
    { ...mutation, source: {} },
    { ...mutation, expected_head: "head" },
    { ...mutation, action: { ...mutation.action, legacy_constraints: null } },
    { ...mutation, action: { ...mutation.action, reference_encoding: "feature_sets_v2" } },
    { ...mutation, action: { kind: "remove", execution: currentPolicy().execution } },
    { ...mutation, action: { kind: "replace", execution: currentPolicy().execution } },
  ]) {
    const { fetcher, calls } = mockNetwork(() => json({ policy: null }));
    await assertRejects(() => savePermissions(
      fetcher, "w/one", "s/one", invalid as SubjectPermissionPolicyMutation,
    ));
    assertEquals(calls, []);
  }
});
Deno.test("first-send CAS conflict and authorization refusals do not reread, rebase or retry", async () => {
  for (const status of [400, 401, 403, 404, 409, 422]) {
    const { fetcher, calls } = mockNetwork(() => json({}, status));
    await assertRejects(
      () => savePermissions(fetcher, "w/one", "s/one", replaceMutation()),
      (error: unknown) => error instanceof PermissionHttpError && error.status === status,
    );
    assertEquals(calls.map((call) => call.method), ["PUT"]);
  }
});
Deno.test("unknown save retries retain exact operation ID, tombstone CAS, payload and Subject after draft changes", async () => {
  const mutation = { ...replaceMutation(), expected_change_id: "policy:removed" };
  const original = JSON.parse(JSON.stringify(mutation));
  const bodies: (BodyInit | null | undefined)[] = [];
  let attempt = 0;
  const { fetcher, calls } = mockNetwork((init) => {
    bodies.push(init?.body);
    if (attempt++ === 0) throw new TypeError("connection lost after send");
    // A retry refusal cannot prove that the original send did not succeed.
    return attempt === 2 ? json({}, 409) : json(mutationResponse());
  });
  const captureUnknown = async (
    save: () => Promise<unknown>,
  ): Promise<PermissionMutationUnknownError> => {
    try {
      await save();
    } catch (error) {
      if (error instanceof PermissionMutationUnknownError) return error;
      throw error;
    }
    throw new Error("missing retained retry");
  };
  const retained = await captureUnknown(() =>
    savePermissions(fetcher, "w/one", "s/one", mutation)
  );
  mutation.operation_id = "later-edit";
  mutation.expected_change_id = "later-cas";
  if (mutation.action.kind === "replace") {
    mutation.action.execution.grants = [];
    mutation.action.delegation.revoked = false;
  }
  const retryRefusal = await captureUnknown(() => retained.retry());
  assertEquals(await retryRefusal.retry(), mutationResponse());
  assertEquals(calls, [0, 1, 2].map(() => ({
    path: subjectPath, method: "PUT", body: original,
  })));
  assertEquals(bodies, [bodies[0], bodies[0], bodies[0]]);
});
Deno.test("5xx, unreadable and invalid success responses retain the request for explicit exact retry", async () => {
  for (const respond of [
    () => json({}, 500),
    () => new Response("not JSON"),
    () => json({ policy: {} }),
    () => json({ policy: null }),
    () => json({ policy: currentPolicy() }),
    () => json({ change_id: "different", policy: currentPolicy() }),
    () => json({ change_id: null, policy: currentPolicy() }),
    () => json({ change_id: undefined, policy: null }),
    () => json({ change_id: "removed", policy: null, can_manage: true }),
    () => new Response("{}", { headers: { "content-length": "8388609" } }),
  ]) {
    const { fetcher, calls } = mockNetwork(respond);
    await assertRejects(
      () => savePermissions(fetcher, "w/one", "s/one", replaceMutation()),
      PermissionMutationUnknownError,
    );
    assertEquals(calls.map((call) => call.method), ["PUT"]);
  }
});
