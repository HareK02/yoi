// Invoked only by the Rust loopback harness. No mocked responses, live server,
// browser fixtures, environment variables, or bearer credentials in argv/logs.
import {
  createDriveClient,
  type DriveClient,
  driveDownloadUrl,
  type DriveEntry,
  type DriveMutation,
  DriveRequestError,
  readBoundedDriveBytes,
} from "../../src/lib/workspace/drive/api.ts";
import {
  createDriveGrant,
  DriveGrantError,
  listDriveGrants,
  revokeDriveGrant,
} from "../../src/lib/workspace/drive-grants/api.ts";
// Preserve the real network implementation before temporarily binding the grant
// adapter's same-origin global fetch to this isolated child/loopback authority.
const networkFetch = globalThis.fetch;

interface Fixture {
  origin: string;
  workspace_id: string;
  owner_token: string;
  second_token: string;
  worker_entry: DriveEntry;
}
function assert(value: unknown, message: string): asserts value {
  if (!value) throw new Error(message);
}
function equal(actual: unknown, expected: unknown, message: string): void {
  assert(JSON.stringify(actual) === JSON.stringify(expected), message);
}
async function rejected(
  call: () => Promise<unknown>,
  code: string,
): Promise<void> {
  try {
    await call();
  } catch (error) {
    assert(
      error instanceof DriveRequestError,
      "rejection must preserve typed Drive API error",
    );
    equal(error.code, code, `expected typed ${code} rejection`);
    equal(
      error.classification,
      "not_committed",
      "rejected request must not look successful/unknown",
    );
    return;
  }
  throw new Error(`expected ${code} rejection`);
}
function authenticatedFetch(origin: string, token: string): typeof fetch {
  const endpoint = new URL(origin);
  assert(
    endpoint.protocol === "http:" && endpoint.hostname === "127.0.0.1",
    "fixture must be loopback only",
  );
  return (input, init) => {
    const path = input instanceof Request ? input.url : String(input);
    const url = new URL(path, endpoint);
    assert(
      url.origin === endpoint.origin && url.pathname.startsWith("/api/"),
      "never send fixture credentials off-origin",
    );
    const headers = new Headers(
      input instanceof Request ? input.headers : undefined,
    );
    new Headers(init && "headers" in init ? init.headers : undefined).forEach((
      value,
      key,
    ) => headers.set(key, value));
    headers.set("authorization", `Bearer ${token}`);
    return networkFetch(url, { ...init, headers, redirect: "error" });
  };
}
async function saved(
  client: DriveClient,
  ws: string,
  request: string,
  mutation: DriveMutation,
): Promise<DriveEntry> {
  const result = await client.mutate(ws, request, mutation);
  assert(result.entry !== null, "creation/update must return live entry");
  return result.entry;
}
async function download(
  fetchFn: typeof fetch,
  ws: string,
  entry: DriveEntry,
): Promise<Uint8Array> {
  const response = await fetchFn(
    driveDownloadUrl(ws, entry.entry, entry.revision),
  );
  assert(response.status === 200, "download must succeed at exact revision");
  equal(
    response.headers.get("x-content-type-options"),
    "nosniff",
    "downloads must not sniff active content",
  );
  assert(
    response.headers.get("content-disposition")?.startsWith("attachment;"),
    "download must be attachment",
  );
  return readBoundedDriveBytes(response, 16 * 1024 * 1024);
}

// Credentials arrive exclusively over the child pipe from a fresh temporary DB.
const fixture: Fixture = JSON.parse(
  await new Response(Deno.stdin.readable).text(),
);
const ws = fixture.workspace_id;
const fetchOwner = authenticatedFetch(fixture.origin, fixture.owner_token);
const owner = createDriveClient(fetchOwner);
const second = createDriveClient(
  authenticatedFetch(fixture.origin, fixture.second_token),
);
const root = await owner.root(ws);
const initial = await owner.list(ws, root.entry);
equal(initial.entries.map((entry) => entry.entry.node_id), [
  fixture.worker_entry.entry.node_id,
], "Worker-created file must appear in the real Web adapter list");
equal(
  (await owner.readText(ws, fixture.worker_entry.entry)).text,
  "# Worker artifact\n",
  "Web adapter must read Worker bytes from LocalFileSystem",
);
// Exercise the exact grant adapter used by the Workers page, not a hand-written
// HTTP facsimile. Only URL resolution/auth are bound; every response is real DB/API.
try {
  globalThis.fetch = fetchOwner;
  const grants = await listDriveGrants(ws);
  assert(
    grants.length === 2,
    "real Worker grants must be listed by the Web adapter",
  );
  const worker = grants[0];
  for (const access of ["read_only", "read_write"] as const) {
    const created = await createDriveGrant(ws, {
      runtime_id: worker.runtime_id,
      worker_id: worker.worker_id,
      access,
    });
    equal(
      created.access,
      access,
      "grant access must not be silently downgraded",
    );
    assert(
      (await listDriveGrants(ws)).some((grant) =>
        grant.grant_id === created.grant_id && !grant.revoked
      ),
      "grant must be durably visible",
    );
    const revoked = await revokeDriveGrant(ws, created);
    assert(
      revoked.revoked &&
        (await listDriveGrants(ws)).some((grant) =>
          grant.grant_id === created.grant_id && grant.revoked
        ),
      "revoke must be durably visible",
    );
  }
  globalThis.fetch = authenticatedFetch(fixture.origin, fixture.second_token);
  try {
    await listDriveGrants(ws);
    throw new Error("member must not manage grants");
  } catch (error) {
    assert(
      error instanceof DriveGrantError && error.outcome === "not_committed",
      "member grant read must be a real authorized rejection",
    );
  }
} finally {
  globalThis.fetch = networkFetch;
}

const folder = await saved(owner, ws, "web-folder", {
  operation: "create_folder",
  parent: root.entry,
  name: "共有資料",
});
equal(
  (await owner.list(ws, folder.entry)).entries,
  [],
  "new folder must be empty",
);
const markdown = await saved(owner, ws, "web-markdown", {
  operation: "create_text",
  parent: folder.entry,
  name: "日本語.md",
  text: "# 初版\n",
  content_type: "text/markdown",
});
equal(
  (await owner.readText(ws, markdown.entry)).text,
  "# 初版\n",
  "real Markdown read must match created bytes",
);
const updated = await saved(owner, ws, "web-edit", {
  operation: "update_text",
  id: markdown.entry,
  expected_revision: markdown.revision,
  text: "# 更新\n",
  content_type: "text/markdown",
});
equal(
  new TextDecoder().decode(await download(fetchOwner, ws, updated)),
  "# 更新\n",
  "updated Markdown download must use real blob bytes",
);
const empty = await saved(owner, ws, "web-empty", {
  operation: "create_text",
  parent: folder.entry,
  name: "empty.txt",
  text: "",
  content_type: "text/plain",
});
equal(
  (await owner.readText(ws, empty.entry)).text,
  "",
  "empty file is not a missing/error response",
);
equal(
  (await download(fetchOwner, ws, empty)).length,
  0,
  "empty file download must succeed with zero bytes",
);
// A real 1x1 PNG, not a claimed media type over arbitrary text.
const png = Uint8Array.from(
  atob(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/l9sAAAAASUVORK5CYII=",
  ),
  (character) => character.charCodeAt(0),
);
const uploaded = await owner.upload(
  ws,
  "web-png",
  new Blob([png], { type: "image/png" }),
  {
    operation: "create",
    parent: folder.entry,
    name: "画像.png",
    content_type: "image/png",
  },
);
assert(uploaded.entry !== null, "PNG upload must publish metadata");
equal(
  uploaded.entry.size,
  png.length,
  "binary byte count must survive actual upload",
);
equal(
  [
    ...new Uint8Array(
      await (await owner.image(ws, uploaded.entry)).arrayBuffer(),
    ),
  ],
  [...png],
  "authenticated image bytes must survive upload, storage and Web adapter download",
);
equal(
  [...(await download(fetchOwner, ws, uploaded.entry))],
  [...png],
  "PNG download must match original bytes",
);
const binaryUpdated = await owner.upload(
  ws,
  "web-empty-to-png",
  new Blob([png]),
  {
    operation: "update",
    id: empty.entry,
    expected_revision: empty.revision,
    content_type: "image/png",
  },
);
assert(binaryUpdated.entry !== null, "binary update must publish metadata");
equal(
  [...await download(fetchOwner, ws, binaryUpdated.entry)],
  [...png],
  "binary update must replace zero-length file bytes",
);
const status = await owner.status(ws, "web-png");
equal(
  status.state,
  "committed",
  "adapter must distinguish publication from transmission",
);
equal(
  status.response,
  uploaded,
  "status must refer to exact real upload receipt",
);
const firstPage = await owner.list(ws, folder.entry, { limit: 1 });
assert(
  firstPage.next_after !== null && firstPage.entries.length === 1,
  "real list must expose a service-issued page cursor",
);
const following = await owner.list(ws, folder.entry, {
  after: firstPage.next_after,
});
equal(
  new Set(
    [...firstPage.entries, ...following.entries].map((entry) =>
      entry.entry.node_id
    ),
  ).size,
  3,
  "paged traversal must contain each file once",
);

// Two authenticated accounts read the same revision, then race real HTTP writes.
const secondRead = await second.readText(ws, updated.entry);
equal(
  secondRead.entry.revision,
  updated.revision,
  "second actor must observe same revision",
);
const drafts = ["# actor A draft\n", "# actor B draft\n"];
const race = await Promise.allSettled(
  [owner, second].map((client, index) =>
    saved(client, ws, `web-race-${index}`, {
      operation: "update_text",
      id: updated.entry,
      expected_revision: updated.revision,
      text: drafts[index],
      content_type: "text/markdown",
    })
  ),
);
equal(
  race.filter((result) => result.status === "fulfilled").length,
  1,
  "same-revision two-actor CAS must have exactly one winner",
);
const loser = race.find((result) => result.status === "rejected");
assert(
  loser?.status === "rejected" && loser.reason instanceof DriveRequestError,
  "loser must expose typed conflict, not silently retry",
);
equal(loser.reason.code, "conflict", "CAS loser must remain a conflict");
const winnerIndex = race.findIndex((result) => result.status === "fulfilled");
const latest = await owner.readText(ws, updated.entry);
equal(
  latest.text,
  drafts[winnerIndex],
  "durable content must be the winning actor's draft",
);
// This API boundary proves no automatic retry/overwrite. UI dirty-draft
// retention is deliberately verified by the separate browser fixture.

// Latest references bind node IDs, not names/parents, and never become public.
const stableUrl = markdown.latest_url;
const renamed = await saved(owner, ws, "web-rename", {
  operation: "relocate",
  id: latest.entry.entry,
  expected_revision: latest.entry.revision,
  parent: folder.entry,
  name: "改名.md",
});
const moved = await saved(owner, ws, "web-move", {
  operation: "relocate",
  id: renamed.entry,
  expected_revision: renamed.revision,
  parent: root.entry,
  name: renamed.name,
});
equal(moved.entry, markdown.entry, "rename/move must preserve node identity");
equal(moved.latest_url, stableUrl, "rename/move must preserve latest URL");
equal(
  (await owner.metadata(ws, markdown.entry)).parent,
  root.entry,
  "original ref must resolve new parent",
);
equal(
  await (await fetchOwner(stableUrl)).text(),
  drafts[winnerIndex],
  "original latest URL must resolve current bytes",
);
const unauthenticated = await fetch(new URL(stableUrl, fixture.origin), {
  redirect: "error",
});
assert(
  unauthenticated.status === 403 || unauthenticated.status === 401,
  "possessing a latest URL must not authorize reads",
);
await unauthenticated.body?.cancel();
await rejected(
  () =>
    owner.metadata(ws, { workspace_id: ws, node_id: "9223372036854775807" }),
  "not_found",
);
const deleted = await owner.mutate(ws, "web-delete", {
  operation: "delete",
  id: moved.entry,
  expected_revision: moved.revision,
});
equal(deleted.entry, null, "delete must remove live metadata");
await rejected(() => owner.readText(ws, moved.entry), "not_found");
const oldUrl = await fetchOwner(stableUrl);
equal(
  oldUrl.status,
  404,
  "deleted stable URL must not resolve a recreated same-name file",
);
await oldUrl.body?.cancel();
const recreated = await saved(owner, ws, "web-recreate", {
  operation: "create_text",
  parent: root.entry,
  name: moved.name,
  text: "recreated",
  content_type: "text/markdown",
});
assert(
  recreated.entry.node_id !== moved.entry.node_id &&
    recreated.latest_url !== stableUrl,
  "same-name recreate must allocate a new ID and latest URL",
);

// The decoded text bound is independent of JSON escape expansion on the wire.
const escapedText = "\u0000".repeat(64 * 1024);
const escaped = await saved(owner, ws, "web-escaped-text", {
  operation: "create_text",
  parent: root.entry,
  name: "escaped.txt",
  text: escapedText,
  content_type: "text/plain",
});
assert(
  (await owner.readText(ws, escaped.entry)).text === escapedText,
  "real API 64 KiB escaped text must roundtrip without a false response-limit failure",
);

// The adapter also handles real current authorization, not just fixture promises.
await rejected(
  () =>
    createDriveClient(
      authenticatedFetch(fixture.origin, "invalid-fixture-token"),
    ).readText(ws, fixture.worker_entry.entry),
  "denied",
);
console.log(
  "real Web adapter roundtrip passed: Worker visibility, folder/Markdown/PNG/empty bytes, CAS, stable URL lifecycle, authorization",
);
