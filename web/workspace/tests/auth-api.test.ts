declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

import {
  addPasskey,
  loadWhoami,
  loginWithLegacyPasskey,
  readBoundedAuthResponseJson,
  registerPasskey,
} from "../src/lib/workspace/auth/api.ts";

import {
  authUser,
  credentialFixture,
  legacyLoginOptions as loginOptions,
  registrationOptions,
} from "../src/lib/workspace/auth/api.test-fixtures.ts";

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

async function withBrowserBoundary(
  fetcher: typeof fetch,
  run: (calls: { create: number; get: number }) => Promise<void>,
) {
  const descriptors = new Map(
    ["fetch", "window", "navigator"].map((key) => [
      key,
      Object.getOwnPropertyDescriptor(globalThis, key),
    ]),
  );
  const calls = { create: 0, get: 0 };
  Object.defineProperties(globalThis, {
    fetch: { configurable: true, writable: true, value: fetcher },
    window: {
      configurable: true,
      value: { location: { origin: "https://yoi.example" } },
    },
    navigator: {
      configurable: true,
      value: {
        credentials: {
          create: (options: CredentialCreationOptions) => {
            calls.create++;
            assertEquals([
              ...new Uint8Array(options.publicKey!.challenge as ArrayBuffer),
            ], [1, 2, 3]);
            assertEquals([
              ...new Uint8Array(options.publicKey!.user.id as ArrayBuffer),
            ], [4, 5, 6]);
            return Promise.resolve(credentialFixture());
          },
          get: (options: CredentialRequestOptions) => {
            calls.get++;
            assertEquals([
              ...new Uint8Array(
                options.publicKey!.allowCredentials![0].id as ArrayBuffer,
              ),
            ], [7, 8, 9]);
            return Promise.resolve(credentialFixture(true));
          },
        },
      },
    },
  });
  try {
    await run(calls);
  } finally {
    for (const [key, descriptor] of descriptors) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  }
}

for (const purpose of ["registration", "addition"] as const) {
  Deno.test(`${purpose} adapter uses only its own options and complete routes with session credentials`, async () => {
    const requests: { path: string; init: RequestInit }[] = [];
    await withBrowserBoundary(async (input, init) => {
      requests.push({ path: String(input), init: init! });
      return Response.json(
        String(input) === "/api/auth/whoami"
          ? {
            actor: { ...authUser, auth_method: "browser_session" },
            authentication_reference: "fixture-authentication",
            session: {
              kind: "authenticated",
              expires_at: "2099-01-01T00:00:00Z",
            },
          }
          : String(input).endsWith("/options")
          ? registrationOptions
          : {
            user: authUser,
            authentication_reference: "fixture-authentication",
          },
      );
    }, async (calls) => {
      const user = purpose === "registration"
        ? await registerPasskey("alice", "Alice")
        : await addPasskey();
      assertEquals(user, {
        user: authUser,
        authentication_reference: "fixture-authentication",
      });
      assertEquals(calls, { create: 1, get: 0 });
    });
    assertEquals(requests.map(({ path }) => path), [
      ...(purpose === "addition" ? ["/api/auth/whoami"] : []),
      `/api/auth/passkeys/${purpose}/options`,
      `/api/auth/passkeys/${purpose}/complete`,
    ]);
    for (const { path, init } of requests) {
      assertEquals(
        init.method ?? "GET",
        path.endsWith("/whoami") ? "GET" : "POST",
      );
      assertEquals(init.credentials, "same-origin");
      assertEquals(init.headers, { "content-type": "application/json" });
    }
    const ceremony = requests.filter(({ path }) => !path.endsWith("/whoami"));
    assertEquals(
      JSON.parse(String(ceremony[0].init.body)),
      purpose === "registration"
        ? {
          handle: "alice",
          display_name: "Alice",
          browser_origin: "https://yoi.example",
        }
        : {
          expected_user_id: authUser.user_id,
          expected_account_id: authUser.account_id,
          expected_authentication_reference: "fixture-authentication",
          browser_origin: "https://yoi.example",
        },
    );
    const complete = JSON.parse(String(ceremony[1].init.body));
    assertEquals(Object.keys(complete).sort(), ["challenge_id", "credential"]);
    assertEquals(complete.challenge_id, registrationOptions.challenge_id);
    assertEquals(complete.credential.response.attestationObject, "AQID");
    assertEquals(complete.credential.response.transports, ["internal"]);
  });
}

Deno.test("addition rejects expired-session completion without falling back to registration", async () => {
  const paths: string[] = [];
  await withBrowserBoundary(async (input) => {
    paths.push(String(input));
    if (String(input).endsWith("/whoami")) {
      return Response.json({
        actor: { ...authUser, auth_method: "browser_session" },
        authentication_reference: "fixture-authentication",
        session: { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" },
      });
    }
    return String(input).endsWith("/options")
      ? Response.json(registrationOptions)
      : new Response(null, { status: 401 });
  }, async () => {
    await assertRejects(addPasskey(), "Passkey complete: rejected (rejected).");
  });
  assertEquals(paths, [
    "/api/auth/whoami",
    "/api/auth/passkeys/addition/options",
    "/api/auth/passkeys/addition/complete",
  ]);
});

Deno.test("existing-handle registration rejection never starts WebAuthn or retries addition", async () => {
  const paths: string[] = [];
  await withBrowserBoundary(async (input) => {
    paths.push(String(input));
    return new Response(null, { status: 409 });
  }, async (calls) => {
    await assertRejects(
      registerPasskey("alice"),
      "Passkey options: rejected (not-sent).",
    );
    assertEquals(calls, { create: 0, get: 0 });
  });
  assertEquals(paths, ["/api/auth/passkeys/registration/options"]);
});

Deno.test("compatibility login requires a nonempty explicit handle before HTTP or WebAuthn", async () => {
  await withBrowserBoundary(() => {
    throw new Error("Unexpected HTTP request");
  }, async (calls) => {
    for (const handle of ["", "  ", "\n\t"]) {
      await assertRejects(
        loginWithLegacyPasskey(handle),
        "User handle is required for legacy passkey login.",
      );
    }
    assertEquals(calls, { create: 0, get: 0 });
  });
});

Deno.test("login sends the explicit trimmed handle and serializes the assertion", async () => {
  const requests: { path: string; init: RequestInit }[] = [];
  await withBrowserBoundary(async (input, init) => {
    requests.push({ path: String(input), init: init! });
    return Response.json(
      requests.length === 1 ? loginOptions : {
        user: authUser,
        authentication_reference: "fixture-authentication",
      },
    );
  }, async (calls) => {
    assertEquals(await loginWithLegacyPasskey(" alice "), {
      user: authUser,
      authentication_reference: "fixture-authentication",
    });
    assertEquals(calls, { create: 0, get: 1 });
  });
  assertEquals(requests.map(({ path }) => path), [
    "/api/auth/passkeys/login/options",
    "/api/auth/passkeys/login/complete",
  ]);
  assertEquals(JSON.parse(String(requests[0].init.body)), {
    handle: "alice",
    browser_origin: "https://yoi.example",
  });
  const complete = JSON.parse(String(requests[1].init.body));
  assertEquals(complete.challenge_id, loginOptions.challenge_id);
  assertEquals(complete.credential.response.signature, "AQID");
  assertEquals(complete.credential.response.userHandle, null);
  assertEquals(requests.map(({ init }) => init.credentials), [
    "same-origin",
    "same-origin",
  ]);
});
