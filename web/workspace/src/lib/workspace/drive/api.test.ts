import {
  createDriveClient,
  DRIVE_FILE_MAX_BYTES,
  DRIVE_RESPONSE_MAX_BYTES,
  driveDownloadUrl,
  DriveRequestError,
  parseDriveApiError,
  parseDriveDecimal,
  parseDriveEntry,
  parseDriveListResponse,
  parseDriveMutationResponse,
  parseDriveReadTextResponse,
  parseDriveRequestStatusResponse,
  readBoundedDriveBytes,
} from "./api.ts";
import { assert, entry, equal, ref, rejects, throws } from "./test-fixtures.ts";

declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

Deno.test("Drive decimal boundary rejects noncanonical or overflow IDs without losing precision", () => {
  for (
    const value of [
      1,
      null,
      "",
      "0",
      "01",
      "+1",
      "-1",
      "1e3",
      "1.0",
      " 1",
      "١",
      "9223372036854775808",
      "999999999999999999999",
    ]
  ) throws(() => parseDriveDecimal(value));
  equal(parseDriveDecimal("9007199254740993"), "9007199254740993");
  equal(parseDriveDecimal("9223372036854775807"), "9223372036854775807");
});
Deno.test("Drive metadata rejects cross-workspace refs and unsafe latest links", () => {
  const good = entry();
  const parsed = parseDriveEntry(good, "alpha");
  equal(parsed.entry, good.entry);
  equal(parsed.last_mutation_id, good.last_mutation_id);
  equal(parsed.latest_url, good.latest_url);
  for (
    const patch of [
      { entry: ref("2", "beta") },
      { parent: ref("1", "beta") },
      { last_mutation_id: 4 },
      { kind: "directory" },
      { size: -1 },
      { size: DRIVE_FILE_MAX_BYTES + 1 },
      { content_type: "text/plain\r\nx:bad" },
      { name: null },
      { size: null },
      ...[
        "https://evil.test/api/w/alpha/drive/download?entry_workspace_id=alpha&id=2",
        "//evil.test/path",
        "javascript:alert(1)",
        "/api/w/beta/drive/download?entry_workspace_id=alpha&id=2",
        "/api/w/alpha/drive/download?entry_workspace_id=beta&id=2",
        "/api/w/alpha/drive/download?entry_workspace_id=alpha&id=3",
        "/api/w/alpha/drive/download?entry_workspace_id=alpha&id=2&token=secret",
        "/api/w/alpha/drive/download?entry_workspace_id=alpha&id=2&id=2",
        "/api/w/alpha/drive/download?entry_workspace_id=alpha&id=2#fragment",
        "/api/w/alpha/drive/../drive/download?entry_workspace_id=alpha&id=2",
      ].map((latest_url) => ({ latest_url })),
    ]
  ) throws(() => parseDriveEntry({ ...good, ...patch }, "alpha"));
  throws(() =>
    parseDriveEntry({ ...entry("1", "alpha", "1", "folder"), size: 0 }, "alpha")
  );
});
Deno.test("Drive pages and text reject malformed elements and multibyte overflow", () => {
  for (
    const value of [
      null,
      { entries: [entry(), null], next_after: null },
      { entries: [entry(), entry()], next_after: null },
      { entries: [], next_after: 2 },
      {
        entries: Array.from({ length: 201 }, (_, i) => entry(String(i + 2))),
        next_after: null,
      },
    ]
  ) throws(() => parseDriveListResponse(value, "alpha"));
  for (
    const patch of [{ truncated: "false" }, { text: "é".repeat(32769) }, {
      entry: entry("1", "alpha", "1", "folder"),
    }]
  ) {
    throws(() =>
      parseDriveReadTextResponse({
        entry: entry(),
        text: "abc",
        truncated: false,
        ...patch,
      }, "alpha")
    );
  }
});
Deno.test("Drive receipts reject mismatched request ids and impossible committed states", () => {
  const response = { request_id: "req", entry: entry() };
  throws(() => parseDriveMutationResponse(response, "alpha", "other"));
  for (
    const v of [
      { request_id: "req", state: "committed", response: null },
      { request_id: "req", state: "uncommitted", response },
      {
        request_id: "req",
        state: "committed",
        response: { ...response, request_id: "other" },
      },
      { request_id: "other", state: "uncommitted", response: null },
    ]
  ) throws(() => parseDriveRequestStatusResponse(v, "alpha", "req"));
  throws(() =>
    parseDriveApiError({
      code: "conflict",
      classification: "unknown",
      message: "bad",
    })
  );
  throws(() =>
    parseDriveApiError({
      code: "outcome_unknown",
      classification: "not_committed",
      message: "bad",
    })
  );
});
Deno.test("Drive read bounds reject announced and observed bodies above 256 KiB", async () => {
  equal(
    (await readBoundedDriveBytes(
      new Response(new Uint8Array(DRIVE_RESPONSE_MAX_BYTES)),
    )).length,
    DRIVE_RESPONSE_MAX_BYTES,
  );
  await rejects(() =>
    readBoundedDriveBytes(
      new Response("{}", {
        headers: { "content-length": String(DRIVE_RESPONSE_MAX_BYTES + 1) },
      }),
    )
  );
  await rejects(() =>
    readBoundedDriveBytes(
      new Response(new Uint8Array(DRIVE_RESPONSE_MAX_BYTES + 1), {
        headers: { "content-length": "1" },
      }),
    )
  );
});
Deno.test("Drive client uses current routes with same-origin authentication and binary SHA256 upload", async () => {
  const blob = new Blob([new Uint8Array([0, 255, 8])]);
  const digest = await crypto.subtle.digest(
    "SHA-256",
    await blob.arrayBuffer(),
  );
  const expectedHash = Array.from(
    new Uint8Array(digest),
    (b) => b.toString(16).padStart(2, "0"),
  ).join("");
  let called = false;
  const client = createDriveClient((input, init) => {
    called = true;
    const url = new URL(String(input), "https://local.test");
    equal(url.pathname, "/api/w/alpha/drive/upload");
    equal(url.searchParams.get("parent_id"), "9007199254740993");
    equal(url.searchParams.get("size"), "3");
    equal(url.searchParams.get("sha256"), expectedHash);
    equal((init as RequestInit | undefined)?.credentials, "same-origin");
    equal((init as RequestInit | undefined)?.cache, "no-store");
    equal((init as RequestInit | undefined)?.redirect, "error");
    equal((init as RequestInit | undefined)?.method, "PUT");
    assert((init as RequestInit | undefined)?.body === blob);
    equal(
      new Headers((init as RequestInit | undefined)?.headers).get(
        "content-type",
      ),
      "application/octet-stream",
    );
    return Promise.resolve(
      Response.json({
        request_id: "req",
        entry: { ...entry(), name: "raw.bin", parent: ref("9007199254740993") },
      }),
    );
  });
  await client.upload("alpha", "req", blob, {
    operation: "create",
    parent: ref("9007199254740993"),
    name: "raw.bin",
    content_type: "application/octet-stream",
  });
  assert(called);
  equal(
    driveDownloadUrl("alpha", ref("9007199254740993"), "9007199254740995"),
    "/api/w/alpha/drive/download?entry_workspace_id=alpha&id=9007199254740993&expected_mutation_id=9007199254740995",
  );
});
Deno.test("Drive upload over 16 MiB is rejected before fetching", async () => {
  let calls = 0;
  const client = createDriveClient(() => {
    ++calls;
    throw new Error("Must not transmit");
  });
  await rejects(() =>
    client.upload(
      "alpha",
      "req",
      new Blob([new Uint8Array(DRIVE_FILE_MAX_BYTES + 1)]),
      {
        operation: "create",
        parent: ref("1"),
        name: "raw.bin",
        content_type: "application/octet-stream",
      },
    )
  );
  equal(calls, 0);
});
Deno.test("Drive mutation network abort or malformed reply remains unknown after fetch invocation", async () => {
  for (
    const fetchFn of [
      () => Promise.reject(new TypeError("Network failure")),
      () => Promise.reject(new DOMException("Aborted", "AbortError")),
      () => Promise.resolve(new Response("not JSON", { status: 502 })),
      () =>
        Promise.resolve(Response.json({ request_id: "wrong", entry: entry() })),
      () =>
        Promise.resolve(
          new Response(new Uint8Array(DRIVE_RESPONSE_MAX_BYTES + 1)),
        ),
    ]
  ) {
    const client = createDriveClient(fetchFn);
    await rejects(
      () =>
        client.mutate("alpha", "req", {
          operation: "delete",
          id: ref(),
          expected_mutation_id: "9007199254740993",
        }),
      (error) => {
        assert(error instanceof DriveRequestError);
        equal(error.classification, "unknown");
      },
    );
  }
});
Deno.test("Drive typed conflict proves not committed and read responses cannot target another node", async () => {
  const client = createDriveClient(() =>
    Promise.resolve(
      Response.json({
        code: "conflict",
        classification: "not_committed",
        message: "conflict",
      }, { status: 409 }),
    )
  );
  await rejects(
    () =>
      client.mutate("alpha", "req", {
        operation: "delete",
        id: ref(),
        expected_mutation_id: "1",
      }),
    (error) => {
      assert(error instanceof DriveRequestError);
      equal(error.classification, "not_committed");
    },
  );
  const wrong = createDriveClient(() =>
    Promise.resolve(
      Response.json({ entry: entry("3"), text: "abc", truncated: false }),
    )
  );
  await rejects(() => wrong.readText("alpha", ref()));
});
Deno.test("Drive image preview fetches authenticated bounded binary rather than metadata URL", async () => {
  const image = { ...entry(), content_type: "image/png" };
  const client = createDriveClient((url, init) => {
    equal(
      String(url),
      driveDownloadUrl("alpha", ref(), image.last_mutation_id),
    );
    equal((init as RequestInit | undefined)?.credentials, "same-origin");
    equal((init as RequestInit | undefined)?.cache, "no-store");
    return Promise.resolve(new Response(new Uint8Array([0, 1, 2])));
  });
  equal((await client.image("alpha", image)).size, 3);
  await rejects(() =>
    client.image("alpha", { ...image, content_type: "image/svg+xml" })
  );
  await rejects(() =>
    client.image("alpha", { ...image, size: DRIVE_RESPONSE_MAX_BYTES + 1 })
  );
  await rejects(() =>
    client.image("alpha", { ...image, content_type: "image/avif" })
  );
});
Deno.test("Drive malformed mutation response target parent name or operation kind remains unknown", async () => {
  const cases: {
    mutation: import("./api.ts").DriveMutation;
    response: unknown;
  }[] = [
    {
      mutation: {
        operation: "update_text",
        id: ref(),
        expected_mutation_id: "1",
        text: "abc",
        content_type: "text/plain",
      },
      response: entry("3"),
    },
    {
      mutation: {
        operation: "update_text",
        id: ref(),
        expected_mutation_id: "1",
        text: "abc",
        content_type: "text/plain",
      },
      response: entry("2", "alpha", "2", "folder"),
    },
    {
      mutation: {
        operation: "relocate",
        id: ref(),
        expected_mutation_id: "1",
        parent: ref("4"),
        name: "moved.txt",
      },
      response: { ...entry(), parent: ref("4"), name: "wrong.txt" },
    },
    {
      mutation: {
        operation: "relocate",
        id: ref(),
        expected_mutation_id: "1",
        parent: ref("4"),
        name: "moved.txt",
      },
      response: { ...entry(), name: "moved.txt" },
    },
    {
      mutation: {
        operation: "relocate",
        id: ref(),
        expected_mutation_id: "1",
        parent: ref("4"),
        name: "moved.txt",
      },
      response: { ...entry("3"), parent: ref("4"), name: "moved.txt" },
    },
    {
      mutation: { operation: "create_folder", parent: ref("4"), name: "new" },
      response: { ...entry("3", "alpha", "1", "folder"), name: "new" },
    },
    {
      mutation: {
        operation: "create_folder",
        parent: ref("1"),
        name: "note.txt",
      },
      response: entry("3"),
    },
    {
      mutation: {
        operation: "create_text",
        parent: ref("1"),
        name: "note.txt",
        text: "abc",
        content_type: "text/plain",
      },
      response: entry("3", "alpha", "1", "folder"),
    },
    {
      mutation: {
        operation: "create_text",
        parent: ref("1"),
        name: "other.txt",
        text: "abc",
        content_type: "text/plain",
      },
      response: entry("3"),
    },
    {
      mutation: {
        operation: "update_text",
        id: ref(),
        expected_mutation_id: "1",
        text: "abc",
        content_type: "text/plain",
      },
      response: null,
    },
    {
      mutation: { operation: "delete", id: ref(), expected_mutation_id: "1" },
      response: entry(),
    },
  ];
  for (const { mutation, response } of cases) {
    const client = createDriveClient(() =>
      Promise.resolve(Response.json({ request_id: "req", entry: response }))
    );
    await rejects(() => client.mutate("alpha", "req", mutation), (error) => {
      assert(error instanceof DriveRequestError);
      equal(error.classification, "unknown");
    });
  }
});
Deno.test("Drive malformed upload update and create identity remains unknown after transmission", async () => {
  const cases: {
    target: import("./api.ts").DriveUploadTarget;
    response: unknown;
  }[] = [
    {
      target: {
        operation: "update",
        id: ref(),
        expected_mutation_id: "1",
        content_type: "text/plain",
      },
      response: entry("3"),
    },
    {
      target: {
        operation: "update",
        id: ref(),
        expected_mutation_id: "1",
        content_type: "text/plain",
      },
      response: entry("2", "alpha", "2", "folder"),
    },
    {
      target: {
        operation: "create",
        parent: ref("4"),
        name: "note.txt",
        content_type: "text/plain",
      },
      response: entry("3"),
    },
    {
      target: {
        operation: "create",
        parent: ref("1"),
        name: "other.txt",
        content_type: "text/plain",
      },
      response: entry("3"),
    },
    {
      target: {
        operation: "create",
        parent: ref("1"),
        name: "note.txt",
        content_type: "text/plain",
      },
      response: entry("3", "alpha", "1", "folder"),
    },
    {
      target: {
        operation: "update",
        id: ref(),
        expected_mutation_id: "1",
        content_type: "text/plain",
      },
      response: null,
    },
  ];
  for (const { target, response } of cases) {
    const client = createDriveClient(() =>
      Promise.resolve(Response.json({ request_id: "req", entry: response }))
    );
    await rejects(
      () => client.upload("alpha", "req", new Blob(["abc"]), target),
      (error) => {
        assert(error instanceof DriveRequestError);
        equal(error.classification, "unknown");
      },
    );
  }
});

Deno.test("Drive bounded text roundtrip accepts worst-case JSON escaping without increasing raster bytes", async () => {
  const text = "\u0000".repeat(64 * 1024);
  const payload = JSON.stringify({
    entry: { ...entry(), size: 64 * 1024 },
    text,
    truncated: false,
  });
  assert(
    new TextEncoder().encode(payload).length > DRIVE_RESPONSE_MAX_BYTES,
    "fixture must cross the old binary-sized wire bound",
  );
  const client = createDriveClient(() =>
    Promise.resolve(new Response(payload))
  );
  const result = await client.readText("alpha", ref());
  assert(
    result.text === text && !result.truncated,
    "valid 64 KiB UTF-8 text must survive JSON escapes",
  );
  equal(result.entry.size, 64 * 1024);
});

Deno.test("Drive committed request identity remains UTF-8 and URL encoded rather than numeric", () => {
  const id = 'write/資料 ?"#&=+';
  const value = { ...entry(), last_mutation_id: id };
  equal(parseDriveEntry(value, "alpha").last_mutation_id, id);
  const url = new URL(
    driveDownloadUrl("alpha", value.entry, id),
    "https://example.test",
  );
  equal(url.searchParams.get("expected_mutation_id"), id);
  equal(url.searchParams.size, 3);
  equal(url.hash, "");
  for (
    const bad of ["", "\nrequest", "\x7f", "é".repeat(65), "x".repeat(129)]
  ) {
    throws(() => parseDriveEntry({ ...value, last_mutation_id: bad }, "alpha"));
    throws(() => driveDownloadUrl("alpha", value.entry, bad));
  }
});
