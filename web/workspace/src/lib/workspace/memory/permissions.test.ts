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
  parseResolution,
  parseSource,
  PermissionHttpError,
  readPermissions,
  referenceLabel,
} from "./permissions.ts";
import {
  permissionResolution,
  permissionSource,
} from "../../../../test/permissions-fixture.ts";
import catalogFixture from "../../../../../../crates/feature-permissions/tests/fixtures/catalog.json" with {
  type: "json",
};
import assignmentFixture from "../../../../../../crates/feature-permissions/tests/fixtures/assignment.json" with {
  type: "json",
};

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
  const assignment = permissionSource().policies[0].execution;
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
Deno.test("source binds Workspace and keeps revoked, excluded and retained domain constraints read-only", () => {
  const source = permissionSource();
  assertEquals(parseSource(source, "w"), source);
  assertEquals(
    parseSource(source, "w").policies[0].legacy_constraints?.execution[0]
      .resource,
    { kind: "ticket", workspace_id: "w", ticket_id: "T-837" },
  );
  assertThrows(() => parseSource(source, "another-workspace"));
  assertThrows(() =>
    parseSource({
      ...source,
      policies: [...source.policies, ...source.policies],
    }, "w")
  );
  assertThrows(() =>
    parseSource({
      ...source,
      policies: [{ ...source.policies[0], execution: [] }],
    }, "w")
  );
});
Deno.test("resolution requires matching Workspace and preserves server origin paths and diagnostics", () => {
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
const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status });
Deno.test("permission read uses policy and origin from one source snapshot without validate or a second read", async () => {
  const source = permissionSource("w/one", "s");
  const calls: { path: string; method: string }[] = [];
  const fetcher = (async (input: unknown, init?: { method?: string }) => {
    calls.push({ path: String(input), method: init?.method ?? "GET" });
    return json(source);
  }) as typeof fetch;
  const view = await readPermissions(fetcher, "w/one", "s");
  assertEquals(calls, [{
    path: "/api/w/w%2Fone/config/adoption",
    method: "GET",
  }]);
  assertEquals(view.source.history_head, "head:adopted");
  assertEquals(
    view.policy?.execution_resolution,
    permissionResolution("w/one"),
  );
  assertEquals(
    view.policy?.delegation_resolution,
    permissionResolution("w/one", true),
  );
});
Deno.test("explicit refresh replaces catalog, assignment and origins together without caching a prior resolution", async () => {
  const first = permissionSource();
  const next = permissionSource();
  next.history_head = "head:revoked";
  next.catalog_digest = "catalog:changed";
  next.policies[0].change_id = "policy:revoked";
  next.policies[0].execution = { grants: [], exclusions: [], revoked: true };
  next.policies[0].execution_resolution = permissionResolution("w", true);
  const replies = [first, next];
  const fetcher =
    (() => Promise.resolve(json(replies.shift()))) as typeof fetch;
  const before = await readPermissions(fetcher, "w", "s");
  const after = await readPermissions(fetcher, "w", "s");
  assertEquals(before.policy?.execution_resolution, permissionResolution());
  assertEquals(after.source.history_head, "head:revoked");
  assertEquals(after.source.catalog_digest, "catalog:changed");
  assertEquals(
    after.policy?.execution_resolution,
    permissionResolution("w", true),
  );
  assertEquals(after.policy?.execution.revoked, true);
  assertEquals(replies, []);
});
Deno.test("missing, malformed or cross-Workspace inline resolution rejects the source rather than falling back to generic validate", async () => {
  const source = permissionSource();
  for (
    const field of ["execution_resolution", "delegation_resolution"] as const
  ) {
    const { [field]: _missing, ...incomplete } = source.policies[0];
    for (
      const policy of [incomplete, {
        ...source.policies[0],
        [field]: permissionResolution("another-workspace"),
      }, {
        ...source.policies[0],
        [field]: { workspace: "w", allowed: {}, diagnostics: null },
      }]
    ) {
      let requests = 0;
      const fetcher = (() => {
        requests++;
        return Promise.resolve(json({ ...source, policies: [policy] }));
      }) as typeof fetch;
      await assertRejects(() => readPermissions(fetcher, "w", "s"));
      assertEquals(requests, 1);
    }
  }
});
Deno.test("non-owner and source failure are not empty policies, missing Subject is never defaulted", async () => {
  for (const status of [401, 403, 404, 500]) {
    await assertRejects(
      () =>
        readPermissions(
          (() => Promise.resolve(json({}, status))) as typeof fetch,
          "w",
          "s",
        ),
      PermissionHttpError,
    );
  }
  const view = await readPermissions(
    (() => Promise.resolve(json(permissionSource()))) as typeof fetch,
    "w",
    "missing",
  );
  assertEquals(view.policy, null);
});
