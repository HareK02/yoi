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
  fetchConfigRevision,
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
  revision: 7,
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
    if (url.includes("/revisions/")) return Promise.resolve(response(snapshot));
    return Promise.resolve(response(tree));
  }) as typeof fetch;

  await fetchConfigTree("w/one", fetcher);
  await fetchConfigRevision("w/one", 7, fetcher);
  await fetchConfigEntry("w/one", "profiles/main.dcdl", fetcher);
  await commitConfigTree("w/one", {
    base_revision: 4,
    base_digest: "sha256:base",
    changes: [],
    entrypoints: [],
  }, fetcher);

  assertEquals(calls.map((call) => call.url), [
    "/api/w/w%2Fone/config/source-tree",
    "/api/w/w%2Fone/config/source-tree/revisions/7",
    "/api/w/w%2Fone/config/source-tree/entries/profiles%2Fmain.dcdl",
    "/api/w/w%2Fone/config/source-tree/commit",
  ]);
  assertEquals(calls[3].init?.method, "POST");
  assert(
    String(calls[3].init?.body).includes('"base_digest":"sha256:base"'),
  );
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

Deno.test("config source API rejects unsafe revisions and oversized entry content", () => {
  assertThrows(
    () =>
      parseWorkspaceConfigTreeResponse({
        ...tree,
        snapshot: { ...snapshot, revision: Number.MAX_SAFE_INTEGER + 1 },
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

Deno.test("config source API surfaces bounded failed evaluation details", async () => {
  const fetcher = (() =>
    Promise.resolve(
      new Response("structured diagnostics", { status: 422 }),
    )) as typeof fetch;
  let message = "";
  try {
    await commitConfigTree("w", {
      base_revision: 1,
      base_digest: "sha256:base",
      changes: [],
      entrypoints: [],
    }, fetcher);
  } catch (error) {
    message = String(error);
  }
  assert(message.includes("structured diagnostics"));
});
