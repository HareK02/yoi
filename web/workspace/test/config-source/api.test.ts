/// <reference lib="deno.ns" />

import { assert, assertEquals } from "jsr:@std/assert";
import {
  commitConfigTree,
  fetchConfigEntry,
  fetchConfigTree,
  previewConfigTree,
} from "../../src/lib/workspace/config-source/api.ts";

function response(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

Deno.test("config source API stays workspace-scoped and separates preview from commit", async () => {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const fetcher = ((input: string | URL | Request, init?: RequestInit) => {
    calls.push({ url: String(input), init });
    return Promise.resolve(response({ ok: true }));
  }) as typeof fetch;

  await fetchConfigTree("w/one", fetcher);
  await fetchConfigEntry("w/one", "profiles/main.dcdl", fetcher);
  await previewConfigTree("w/one", { changes: [], entrypoints: [] }, fetcher);
  await commitConfigTree("w/one", {
    base_revision: 4,
    base_digest: "sha256:base",
    changes: [],
    entrypoints: [],
  }, fetcher);

  assertEquals(calls.map((call) => call.url), [
    "/api/w/w%2Fone/config/source-tree",
    "/api/w/w%2Fone/config/source-tree/entries/profiles%2Fmain.dcdl",
    "/api/w/w%2Fone/config/source-tree/preview",
    "/api/w/w%2Fone/config/source-tree/commit",
  ]);
  assertEquals(calls[2].init?.method, "POST");
  assertEquals(calls[3].init?.method, "POST");
  assert(
    String(calls[3].init?.body).includes('"base_digest":"sha256:base"'),
  );
});

Deno.test("config source API surfaces failed evaluation instead of treating it as a draft write", async () => {
  const fetcher = (() =>
    Promise.resolve(
      new Response("structured diagnostics", { status: 422 }),
    )) as typeof fetch;
  let message = "";
  try {
    await previewConfigTree("w", { changes: [], entrypoints: [] }, fetcher);
  } catch (error) {
    message = String(error);
  }
  assert(message.includes("structured diagnostics"));
});
