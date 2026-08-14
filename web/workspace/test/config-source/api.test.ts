/// <reference lib="deno.ns" />

import { assert, assertEquals } from "jsr:@std/assert";
import {
  commitConfigTree,
  fetchConfigEntry,
  fetchConfigRevision,
  fetchConfigTree,
} from "../../src/lib/workspace/config-source/api.ts";

function response(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

Deno.test("config source API commits directly through the workspace scope", async () => {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const fetcher = ((input: string | URL | Request, init?: RequestInit) => {
    calls.push({ url: String(input), init });
    return Promise.resolve(response({ ok: true }));
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

Deno.test("config source API surfaces failed evaluation instead of treating it as a successful commit", async () => {
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
