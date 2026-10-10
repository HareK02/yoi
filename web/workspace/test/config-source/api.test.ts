/// <reference lib="deno.ns" />

import {
  assert,
  assertEquals,
  assertRejects,
  assertThrows,
} from "jsr:@std/assert";
import {
  commitConfigTree,
  ConfigSourceApiError,
  fetchConfigEntry,
  fetchConfigHistory,
  fetchConfigTree,
  parseWorkspaceConfigTreeResponse,
} from "../../src/lib/workspace/config-source/api.ts";

function response(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

const entry = {
  path: "profiles/main.dcdl",
  content_type: "decodal",
  content: "profile = {}",
  content_digest: "sha256:entry",
};
const snapshot = {
  digest: "sha256:tree",
  entries: { "profiles/main.dcdl": entry },
};
const tree = {
  snapshot,
  contract: {
    contract_version: 1,
    decodal_version: "0.4.0",
    schema_version: 1,
    entrypoints: ["main.dcdl"],
    import_policy_version: 1,
    schema_bundle: {
      contributions: [],
      source: "builtin",
      fingerprint: "sha256:schema",
    },
    fingerprint: "sha256:toolchain",
  },
  projection_digest: "sha256:projection",
};

Deno.test("config source API commits directly through the workspace scope", async () => {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const fetcher = ((input: string | URL | Request, init?: RequestInit) => {
    const url = String(input);
    calls.push({ url, init });
    if (url.includes("/entries/")) return Promise.resolve(response(entry));
    if (url.includes("/history/")) return Promise.resolve(response(snapshot));
    return Promise.resolve(response(tree));
  }) as typeof fetch;

  await fetchConfigTree("w/one", fetcher);
  await fetchConfigHistory("w/one", "sha256:tree", fetcher);
  await fetchConfigEntry("w/one", "profiles/main.dcdl", fetcher);
  await commitConfigTree("w/one", {
    base_digest: "sha256:base",
    changes: [],
    entrypoints: [],
  }, fetcher);

  assertEquals<unknown>(calls.map((call) => call.url), [
    "/api/w/w%2Fone/config/source-tree",
    "/api/w/w%2Fone/config/source-tree/history/sha256%3Atree",
    "/api/w/w%2Fone/config/source-tree/entries/profiles%2Fmain.dcdl",
    "/api/w/w%2Fone/config/source-tree/commit",
  ]);
  assertEquals<unknown>(calls[3].init?.method, "POST");
  assert(
    String(calls[3].init?.body).includes('"base_digest":"sha256:base"'),
  );
  assertEquals(JSON.parse(String(calls[3].init?.body)), {
    base_digest: "sha256:base", changes: [], entrypoints: [],
  });
});

function withSchema(source: unknown, contributionSource: unknown = "builtin") {
  return {
    ...tree,
    contract: {
      ...tree.contract,
      schema_bundle: {
        ...tree.contract.schema_bundle,
        source,
        contributions: [{
          provider_id: "builtin",
          namespace: "workspace",
          version: "1",
          source: contributionSource,
          source_digest: "sha256:contribution",
        }],
      },
    },
  };
}

for (const field of ["bundle", "contribution"] as const) {
  Deno.test(`config source API accepts ${field} schema bodies larger than metadata`, async () => {
    const source = "schema body\n".repeat(6000);
    const body = field === "bundle"
      ? withSchema(source)
      : withSchema("builtin", source);
    const fetcher = (() => Promise.resolve(response(body))) as typeof fetch;
    assertEquals<unknown>(await fetchConfigTree("w", fetcher), body);
    assertEquals<unknown>(
      await commitConfigTree("w", {
        base_digest: snapshot.digest,
        changes: [],
        entrypoints: ["main.dcdl"],
      }, fetcher),
      body,
    );
  });

  Deno.test(`config source API bounds ${field} schema bodies in UTF-8 bytes and checks types`, () => {
    const body = (source: unknown) =>
      field === "bundle" ? withSchema(source) : withSchema("builtin", source);
    const limit = 8 * 1024 * 1024;
    const atLimit = "é".repeat(limit / 2);
    assertEquals<unknown>(
      parseWorkspaceConfigTreeResponse(body(atLimit)),
      body(atLimit),
    );
    for (const invalid of [atLimit + "x", null, 42, {}]) {
      assertThrows(
        () => parseWorkspaceConfigTreeResponse(body(invalid)),
        ConfigSourceApiError,
      );
    }
  });
}

Deno.test("config source API retains metadata and total response limits", async () => {
  const invalid = withSchema("builtin");
  invalid.contract.schema_bundle.contributions[0].provider_id = "x".repeat(
    4097,
  );
  assertThrows(
    () => parseWorkspaceConfigTreeResponse(invalid),
    ConfigSourceApiError,
  );
  for (
    const headers of [
      new Headers(),
      new Headers({ "content-length": String(8 * 1024 * 1024 + 1) }),
    ]
  ) {
    const fetcher = (() =>
      Promise.resolve(
        new Response(
          JSON.stringify(withSchema("x".repeat(8 * 1024 * 1024))),
          { headers },
        ),
      )) as typeof fetch;
    await assertRejects(
      () => fetchConfigTree("w", fetcher),
      ConfigSourceApiError,
      "too large",
    );
  }
});

Deno.test("config source API rejects unknown response fields", async () => {
  const fetcher =
    (() =>
      Promise.resolve(response({ ...tree, unexpected: true }))) as typeof fetch;
  await assertRejects(
    () => fetchConfigTree("w", fetcher),
    ConfigSourceApiError,
    "invalid response",
  );
});

Deno.test("config source API rejects mismatched entry map paths", async () => {
  const invalid = {
    ...tree,
    snapshot: {
      ...snapshot,
      entries: { "profiles/other.dcdl": entry },
    },
  };
  const fetcher = (() => Promise.resolve(response(invalid))) as typeof fetch;
  await assertRejects(
    () => fetchConfigTree("w", fetcher),
    ConfigSourceApiError,
    "invalid response",
  );
});

Deno.test("config source API rejects removed snapshot revision fields and oversized entry content", () => {
  assertThrows(
    () =>
      parseWorkspaceConfigTreeResponse({
        ...tree,
        snapshot: { ...snapshot, revision: 7 },
      }),
    ConfigSourceApiError,
    "invalid response",
  );
  assertThrows(
    () =>
      parseWorkspaceConfigTreeResponse({
        ...tree,
        snapshot: {
          ...snapshot,
          entries: {
            "profiles/main.dcdl": {
              ...entry,
              content: "x".repeat(256 * 1024 + 1),
            },
          },
        },
      }),
    ConfigSourceApiError,
    "invalid response",
  );
});

Deno.test("config source API does not materialize imported builtins in the workspace file list", async () => {
  const body = {
    ...tree,
    snapshot: {
      ...snapshot,
      entries: {
        "profiles/main.dcdl": {
          ...entry,
          content: 'import "$builtin/profiles/companion.dcdl"',
        },
      },
    },
  };
  const fetcher = (() => Promise.resolve(response(body))) as typeof fetch;
  const result = await fetchConfigTree("w", fetcher);
  assertEquals(Object.keys(result.snapshot.entries), ["profiles/main.dcdl"]);
  assertEquals(
    result.snapshot.entries["profiles/main.dcdl"].content,
    body.snapshot.entries["profiles/main.dcdl"].content,
  );
});

Deno.test("config source API preserves unknown read-only builtin diagnostics without fallback requests", async () => {
  const diagnostic = {
    path: "main.dcdl",
    tree_digest: "sha256:tree",
    kind: "import",
    span: { start_byte: 8, end_byte: 40 },
    message:
      "unknown or non-public read-only builtin source: $builtin/profiles/missing.dcdl",
    labels: [],
    notes: [],
  };
  const calls: string[] = [];
  const fetcher = ((input: string | URL | Request) => {
    calls.push(String(input));
    return Promise.resolve(
      response({ error: JSON.stringify([diagnostic]) }, 400),
    );
  }) as typeof fetch;
  const error = await assertRejects(
    () =>
      commitConfigTree("w", {
        base_digest: "sha256:tree",
        changes: [],
        entrypoints: ["main.dcdl"],
      }, fetcher),
    ConfigSourceApiError,
    diagnostic.message,
  );
  assert(String(error).includes("main.dcdl"));
  assertEquals(calls, ["/api/w/w/config/source-tree/commit"]);
});

Deno.test("config source API surfaces bounded failed evaluation details", async () => {
  const fetcher = (() =>
    Promise.resolve(
      new Response("structured diagnostics", { status: 422 }),
    )) as typeof fetch;
  let message = "";
  try {
    await commitConfigTree("w", {
      base_digest: "sha256:base",
      changes: [],
      entrypoints: [],
    }, fetcher);
  } catch (error) {
    message = String(error);
  }
  assert(message.includes("structured diagnostics"));
});

Deno.test("config source API carries bounded optional authoring schemas without changing validation source", () => {
  const body = withSchema("validation");
  const authoringSource = "shape\n".repeat(6000);
  const contribution = {
    ...body.contract.schema_bundle.contributions[0],
    authoring_source: authoringSource,
  };
  const withAuthoring = {
    ...body,
    contract: {
      ...body.contract,
      schema_bundle: {
        ...body.contract.schema_bundle,
        contributions: [contribution],
      },
    },
  };
  const parsed = parseWorkspaceConfigTreeResponse(withAuthoring);
  assertEquals(
    parsed.contract.schema_bundle.contributions[0].authoring_source,
    authoringSource,
  );
  assertEquals(parsed.contract.schema_bundle.source, "validation");
  for (const invalid of [null, 42, {}, "é".repeat(4 * 1024 * 1024) + "x"]) {
    assertThrows(() =>
      parseWorkspaceConfigTreeResponse({
        ...withAuthoring,
        contract: {
          ...body.contract,
          schema_bundle: {
            ...body.contract.schema_bundle,
            contributions: [{ ...contribution, authoring_source: invalid }],
          },
        },
      }), ConfigSourceApiError);
  }
});
