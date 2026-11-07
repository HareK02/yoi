/// <reference lib="deno.ns" />

import {
  assert,
  assertEquals,
  assertRejects,
  assertThrows,
} from "jsr:@std/assert";
import {
  commitConfigTree,
  type ConfigurationEditingState,
  ConfigSourceApiError,
  fetchConfigEntry,
  fetchConfigHistory,
  fetchConfigTree,
  parseWorkspaceConfigTreeResponse,
  previewConfigTree,
  reconcileConfigTree,
} from "../../src/lib/workspace/config-source/api.ts";
import type {
  ConfigurationAdoptionReceipt,
  ConfigurationAdoptionRequest,
  ConfigurationPreviewResponse,
} from "../../src/lib/generated/configuration-adoption.generated.ts";
import type {
  ConfigEntry,
  WorkspaceConfigTreeResponse,
} from "../../src/lib/workspace/config-source/types.ts";

function response(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

const entry: ConfigEntry = {
  path: "profiles/main.dcdl",
  content_type: "decodal",
  content: "profile = {}",
  content_digest: "sha256:entry",
};
const snapshot = {
  digest: "sha256:tree",
  entries: { "profiles/main.dcdl": entry },
};
const tree: WorkspaceConfigTreeResponse = {
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

function editingState(
  state: WorkspaceConfigTreeResponse = tree,
): ConfigurationEditingState {
  return {
    state,
    history_head: "head:observed",
    catalog_digest: "sha256:catalog",
    expected_legacy_export: "sha256:legacy",
    source_stored: false,
  };
}

function sourceResponse(state: unknown = tree) {
  return {
    ...editingState(),
    state,
    catalog: {},
    policies: [],
    migration_diagnostics: [],
  };
}

function adoptionRequest(
  source: ConfigurationAdoptionRequest["source"] = {
    base_digest: snapshot.digest,
    changes: [],
    entrypoints: ["main.dcdl"],
  },
): ConfigurationAdoptionRequest {
  const observed = editingState();
  return {
    operation_id: "operation:one",
    expected_head: observed.history_head,
    expected_catalog_digest: observed.catalog_digest,
    expected_legacy_export: observed.expected_legacy_export,
    source,
  };
}

function adoptionReceipt(
  workspaceId: string,
  request: ConfigurationAdoptionRequest,
  state: WorkspaceConfigTreeResponse = tree,
): ConfigurationAdoptionReceipt {
  return {
    workspace_id: workspaceId,
    operation_id: request.operation_id,
    history_head: "head:adopted",
    state,
    source_stored: true,
    policy_committed: true,
    runtime_application: "next_current_policy_check",
  };
}

Deno.test("config source API loads adoption metadata and posts the full request while retaining history and entry routes", async () => {
  const request = adoptionRequest({
    base_digest: snapshot.digest,
    changes: [{
      kind: "update",
      path: entry.path,
      expected_digest: entry.content_digest,
      content: "profile = { changed = true }",
    }],
    entrypoints: ["main.dcdl"],
  });
  const receipt = adoptionReceipt("w/one", request);
  const preview: ConfigurationPreviewResponse = {
    expected_head: request.expected_head,
    state: tree,
    changed_paths: [entry.path],
    changed_subjects: ["subject:one"],
  };
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const fetcher = ((input: string | URL | Request, init?: RequestInit) => {
    const url = String(input);
    calls.push({ url, init });
    if (url.includes("/entries/")) return Promise.resolve(response(entry));
    if (url.includes("/history/")) return Promise.resolve(response(snapshot));
    if (url.endsWith("/preview")) return Promise.resolve(response(preview));
    if (url.endsWith("/commit")) return Promise.resolve(response(receipt));
    if (url.endsWith("/reconcile")) {
      return Promise.resolve(response({ status: "adopted", receipt }));
    }
    return Promise.resolve(response(sourceResponse()));
  }) as typeof fetch;

  assertEquals(await fetchConfigTree("w/one", fetcher), editingState());
  assertEquals(await fetchConfigHistory("w/one", "sha256:tree", fetcher), snapshot);
  assertEquals(await fetchConfigEntry("w/one", "profiles/main.dcdl", fetcher), entry);
  assertEquals(await previewConfigTree("w/one", request, fetcher), preview);
  assertEquals(await commitConfigTree("w/one", request, fetcher), receipt);
  assertEquals(await reconcileConfigTree("w/one", request, fetcher), {
    status: "adopted",
    receipt,
  });

  assertEquals(calls.map((call) => call.url), [
    "/api/w/w%2Fone/config/adoption",
    "/api/w/w%2Fone/config/source-tree/history/sha256%3Atree",
    "/api/w/w%2Fone/config/source-tree/entries/profiles%2Fmain.dcdl",
    "/api/w/w%2Fone/config/adoption/preview",
    "/api/w/w%2Fone/config/adoption/commit",
    "/api/w/w%2Fone/config/adoption/reconcile",
  ]);
  for (const call of calls.slice(3)) {
    assertEquals(call.init?.method, "POST");
    assertEquals(
      new Headers(call.init?.headers).get("content-type"),
      "application/json",
    );
    assertEquals(JSON.parse(String(call.init?.body)), request);
  }
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
    const request = adoptionRequest();
    const receipt = { ...adoptionReceipt("w", request), state: body };
    const fetcher = ((input: string | URL | Request) =>
      Promise.resolve(response(
        String(input).endsWith("/commit") ? receipt : sourceResponse(body),
      ))) as typeof fetch;
    assertEquals<unknown>(await fetchConfigTree("w", fetcher), {
      ...editingState(),
      state: body,
    });
    assertEquals<unknown>(
      await commitConfigTree("w", request, fetcher),
      receipt,
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
          JSON.stringify(sourceResponse(withSchema("x".repeat(8 * 1024 * 1024)))),
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
  const fetcher = (() =>
    Promise.resolve(response(
      sourceResponse({ ...tree, unexpected: true }),
    ))) as typeof fetch;
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
  const fetcher = (() => Promise.resolve(response(sourceResponse(invalid)))) as typeof fetch;
  await assertRejects(
    () => fetchConfigTree("w", fetcher),
    ConfigSourceApiError,
    "invalid response",
  );
});

Deno.test("config source API rejects unknown snapshot fields and oversized entry content", () => {
  assertThrows(
    () =>
      parseWorkspaceConfigTreeResponse({
        ...tree,
        snapshot: { ...snapshot, unexpected_field: true },
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
  const fetcher = (() => Promise.resolve(response(sourceResponse(body)))) as typeof fetch;
  const result = await fetchConfigTree("w", fetcher);
  assertEquals(Object.keys(result.state.snapshot.entries), ["profiles/main.dcdl"]);
  assertEquals(
    result.state.snapshot.entries["profiles/main.dcdl"].content,
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
      commitConfigTree("w", adoptionRequest(), fetcher),
    ConfigSourceApiError,
    diagnostic.message,
  );
  assert(String(error).includes("main.dcdl"));
  assertEquals(calls, ["/api/w/w/config/adoption/commit"]);
});

Deno.test("config source API surfaces bounded failed evaluation details", async () => {
  const fetcher = (() =>
    Promise.resolve(
      new Response("structured diagnostics", { status: 422 }),
    )) as typeof fetch;
  const error = await assertRejects(
    () => commitConfigTree("w", adoptionRequest(), fetcher),
    ConfigSourceApiError,
    "structured diagnostics",
  );
  assertEquals(error.status, 422);
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

Deno.test("config source API previews adoption change scope without calling commit", async () => {
  const request = adoptionRequest();
  const preview: ConfigurationPreviewResponse = {
    expected_head: request.expected_head,
    state: tree,
    changed_paths: [entry.path],
    changed_subjects: ["subject:one"],
  };
  const calls: string[] = [];
  const result = await previewConfigTree(
    "w/one",
    request,
    ((input: string | URL | Request, init?: RequestInit) => {
      calls.push(String(input));
      assertEquals(init?.method, "POST");
      assertEquals(JSON.parse(String(init?.body)), request);
      return Promise.resolve(response(preview));
    }) as typeof fetch,
  );
  assertEquals(calls, ["/api/w/w%2Fone/config/adoption/preview"]);
  assertEquals(result, preview);
});
