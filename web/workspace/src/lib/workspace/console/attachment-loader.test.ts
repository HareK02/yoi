import {
  clearConsoleAttachmentCache,
  ConsoleAttachmentFetchError,
  loadConsoleAttachment,
} from "./attachment-loader.ts";

declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

function assertEquals(actual: unknown, expected: unknown): void {
  if (actual !== expected) {
    throw new Error(`expected ${String(expected)}, got ${String(actual)}`);
  }
}

function assertStrictEquals(actual: unknown, expected: unknown): void {
  if (actual !== expected) {
    throw new Error("expected values to be strictly identical");
  }
}

Deno.test("console attachment loads are lazy-callable and deduplicate in-flight and successful fetches", async () => {
  clearConsoleAttachmentCache();
  let calls = 0;
  let release!: () => void;
  const gate = new Promise<void>((resolve) => release = resolve);
  const fetcher = (async () => {
    calls += 1;
    await gate;
    return new Response(new Uint8Array([1, 2, 3]), {
      headers: { "content-type": "image/png" },
    });
  }) as typeof fetch;

  assertEquals(calls, 0);
  const first = loadConsoleAttachment("/attachment/a", fetcher);
  const second = loadConsoleAttachment("/attachment/a", fetcher);
  assertStrictEquals(first, second);
  assertEquals(calls, 1);
  release();
  const blob = await first;
  assertEquals(blob.size, 3);
  assertStrictEquals(
    await loadConsoleAttachment("/attachment/a", fetcher),
    blob,
  );
  assertEquals(calls, 1);
});

Deno.test("console attachment failures stay local and classify retention and authorization", async () => {
  clearConsoleAttachmentCache();
  for (
    const [status, kind] of [[404, "missing"], [410, "expired"], [
      403,
      "unauthorized",
    ]] as const
  ) {
    try {
      await loadConsoleAttachment(
        `/attachment/${status}`,
        (async () => new Response(null, { status })) as typeof fetch,
      );
      throw new Error("expected failure");
    } catch (error) {
      assertEquals(
        error instanceof ConsoleAttachmentFetchError && error.kind,
        kind,
      );
    }
  }
});
