import { assert, assertEquals, assertRejects } from "@std/assert";
import { createDriveFixture, fixtureRootId } from "../../../../tools/web-ux/drive-fixture/api.ts";
import { createDriveClient, DriveRequestError } from "../../src/lib/workspace/drive/api.ts";
function setup() {
  const fixture = createDriveFixture();
  const fetchFixture: typeof fetch = async (input, init) => {
    const request = new Request(new URL(String(input), "http://127.0.0.1"), init);
    const response = await fixture.handler(request);
    if (!response) throw new Error(`Unimplemented fixture ${request.url}`);
    return response;
  };
  return { fixture, client: createDriveClient(fetchFixture), fetchFixture };
}
const ref = (workspace_id: string, node_id: string) => ({ workspace_id, node_id });
Deno.test("browser fixture preserves decimal identity, bounded reads, binary and page cursors", async () => {
  const { client, fetchFixture } = setup();
  const root = await client.root("drive-paged");
  assertEquals(root.entry.node_id, fixtureRootId);
  const first = await client.list("drive-paged", root.entry, { limit: 100 });
  const second = await client.list("drive-paged", root.entry, {
    limit: 100,
    after: first.next_after,
  });
  assertEquals(first.entries.length, 100);
  assertEquals(second.entries.length, 100);
  assert(
    !second.entries.some((entry) =>
      first.entries.some((other) => other.entry.node_id === entry.entry.node_id)
    ),
  );
  const text = await client.readText("drive-paged", ref("drive-paged", "9"));
  assertEquals(text.truncated, true);
  assert(new TextEncoder().encode(text.text).length <= 65536);
  const image = await client.metadata("drive-paged", ref("drive-paged", "5"));
  const imageBytes = await client.image("drive-paged", image);
  assertEquals(imageBytes.size, image.size);
  assert(imageBytes.size < 256 * 1024);
  const png = new Uint8Array(await imageBytes.arrayBuffer());
  const dimensions = new DataView(png.buffer);
  assertEquals([dimensions.getUint32(16), dimensions.getUint32(20)], [1200, 800]);
  const escapedWire = await fetchFixture(
    "/api/w/drive-paged/drive/read-text?entry_workspace_id=drive-paged&id=11&max_bytes=65536",
  );
  const escapedLength = (await escapedWire.arrayBuffer()).byteLength;
  assert(escapedLength > 256 * 1024 && escapedLength < 512 * 1024);
  const escaped = await client.readText("drive-paged", ref("drive-paged", "11"));
  assertEquals(escaped.text, "\u0000".repeat(65536));
  assertEquals(escaped.truncated, false);
});
Deno.test("browser fixture two revisions conflict without committing the losing request", async () => {
  const { client } = setup();
  const entry = await client.metadata("home-owner", ref("home-owner", "3"));
  await client.mutate("home-owner", "tab-one", {
    operation: "update_text",
    id: entry.entry,
    expected_revision: entry.revision,
    text: "first actor",
    content_type: "text/markdown",
  });
  const failure = await assertRejects(
    () =>
      client.mutate("home-owner", "tab-two", {
        operation: "update_text",
        id: entry.entry,
        expected_revision: entry.revision,
        text: "second actor draft",
        content_type: "text/markdown",
      }),
    DriveRequestError,
  );
  assertEquals((failure as DriveRequestError).code, "conflict");
  assertEquals((await client.status("home-owner", "tab-one")).state, "committed");
  assertEquals((await client.status("home-owner", "tab-two")).state, "uncommitted");
  assertEquals((await client.readText("home-owner", entry.entry)).text, "first actor");
});
Deno.test("browser fixture upload unknown is reconciled by receipt and denied writes never commit", async () => {
  const { client, fetchFixture } = setup();
  await fetchFixture("/__fixture/fault", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ workspace: "home-owner", mode: "unknown" }),
  });
  const root = await client.root("home-owner");
  const failure = await assertRejects(
    () =>
      client.upload("home-owner", "upload-one", new Blob(["upload bytes"]), {
        operation: "create",
        parent: root.entry,
        name: "upload.txt",
        content_type: "text/plain",
      }),
    DriveRequestError,
  );
  assertEquals((failure as DriveRequestError).code, "outcome_unknown");
  assertEquals((await client.status("home-owner", "upload-one")).state, "committed");
  const memberRoot = await client.root("home-member");
  const denied = await assertRejects(
    () =>
      client.mutate("home-member", "member-one", {
        operation: "create_folder",
        parent: memberRoot.entry,
        name: "denied",
      }),
    DriveRequestError,
  );
  assertEquals((denied as DriveRequestError).code, "denied");
  assertEquals((await client.status("home-member", "member-one")).state, "uncommitted");
});
Deno.test("browser fixture rename and move retain identity while delete and recreate do not reuse links", async () => {
  const { client } = setup();
  let entry = await client.metadata("home-owner", ref("home-owner", "3"));
  const latest = entry.latest_url;
  const moved = await client.mutate("home-owner", "move", {
    operation: "relocate",
    id: entry.entry,
    expected_revision: entry.revision,
    parent: ref("home-owner", "2"),
    name: "renamed.md",
  });
  entry = moved.entry!;
  assertEquals(entry.latest_url, latest);
  await client.mutate("home-owner", "delete", {
    operation: "delete",
    id: entry.entry,
    expected_revision: entry.revision,
  });
  const replacement = await client.mutate("home-owner", "recreate", {
    operation: "create_text",
    parent: ref("home-owner", "2"),
    name: "renamed.md",
    text: "new entity",
    content_type: "text/markdown",
  });
  assert(replacement.entry!.entry.node_id !== entry.entry.node_id);
  const missing = await assertRejects(
    () => client.metadata("home-owner", entry.entry),
    DriveRequestError,
  );
  assertEquals((missing as DriveRequestError).code, "not_found");
});
