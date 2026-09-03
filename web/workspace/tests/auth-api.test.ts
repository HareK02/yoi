declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

import {
  loadWhoami,
  readBoundedAuthResponseJson,
} from "../src/lib/workspace/auth/api.ts";

function assertEquals(actual: unknown, expected: unknown): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

async function assertRejects(
  promise: Promise<unknown>,
  expectedMessage: string,
  forbiddenContent?: string,
): Promise<void> {
  try {
    await promise;
  } catch (error) {
    if (!(error instanceof Error) || error.message !== expectedMessage) {
      throw new Error("unexpected rejection");
    }
    if (
      forbiddenContent !== undefined && error.message.includes(forbiddenContent)
    ) {
      throw new Error("diagnostic leaked response content");
    }
    return;
  }
  throw new Error("expected rejection");
}

Deno.test("bounded auth response reader parses a valid JSON object", async () => {
  const response = new Response('{"status":"pending"}', {
    headers: { "content-type": "application/json" },
  });
  assertEquals(await readBoundedAuthResponseJson(response), {
    status: "pending",
  });
});

Deno.test("bounded auth response reader rejects declared and streamed oversize bodies", async () => {
  await assertRejects(
    readBoundedAuthResponseJson(
      new Response("{}", { headers: { "content-length": "262145" } }),
    ),
    "Invalid auth response: response body exceeds the size limit.",
  );

  const chunk = new Uint8Array(131_073);
  const response = new Response(
    new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(chunk);
        controller.enqueue(chunk);
        controller.close();
      },
    }),
  );
  await assertRejects(
    readBoundedAuthResponseJson(response),
    "Invalid auth response: response body exceeds the size limit.",
  );
});

Deno.test("auth requests do not expose non-success response content", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = () =>
    Promise.resolve(
      new Response('{"message":"access-secret"}', {
        status: 401,
        headers: { "content-type": "application/json" },
      }),
    );
  try {
    await assertRejects(
      loadWhoami(),
      "Auth request failed (401).",
      "access-secret",
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});

Deno.test("bounded auth response reader rejects invalid JSON without echoing content", async () => {
  const sensitive = "access-secret";
  await assertRejects(
    readBoundedAuthResponseJson(new Response(`{${sensitive}`)),
    "Invalid auth response: response body is not valid JSON.",
    sensitive,
  );
});
