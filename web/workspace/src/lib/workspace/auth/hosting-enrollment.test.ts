declare const Deno: { test(name: string, fn: () => Promise<void>): void };

import { enrollHostingPasskey } from "./hosting-enrollment.ts";
import {
  authUser,
  credentialFixture,
  registrationOptions,
} from "./api.test-fixtures.ts";

function equal(actual: unknown, expected: unknown): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error("Values differ");
  }
}
async function rejects(run: Promise<unknown>): Promise<void> {
  try {
    await run;
  } catch (error) {
    equal(
      error instanceof Error && error.message,
      "Passkey enrollment could not be confirmed.",
    );
    return;
  }
  throw new Error("Expected rejection");
}

// 本番clientとWebAuthn変換を接続し、fetchとbrowser credential境界だけを代替する。
// ServerのBearer認証・署名・cookie発行を保証する試験ではない。
async function boundary(
  fetcher: typeof fetch,
  run: () => Promise<void>,
  create: (options: CredentialCreationOptions) => Promise<Credential | null> =
    async () => credentialFixture(),
) {
  const descriptors = new Map(
    ["fetch", "window", "navigator"].map((
      key,
    ) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]),
  );
  Object.defineProperties(globalThis, {
    fetch: { configurable: true, writable: true, value: fetcher },
    window: {
      configurable: true,
      value: {
        isSecureContext: true,
        location: { origin: "https://yoi.example" },
      },
    },
    navigator: { configurable: true, value: { credentials: { create } } },
  });
  try {
    await run();
  } finally {
    for (const [key, descriptor] of descriptors) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  }
}

Deno.test("host enrollment sends bearer only to same-origin addition endpoints, never registration", async () => {
  const requests: { path: string; init: RequestInit }[] = [];
  const controller = new AbortController();
  await boundary(async (input, init) => {
    requests.push({ path: String(input), init: init! });
    return Response.json(
      String(input).endsWith("/options")
        ? registrationOptions
        : { user: authUser },
    );
  }, async () => {
    equal(
      await enrollHostingPasskey("host-test-token", controller.signal),
      authUser,
    );
    equal(requests.map((request) => request.path), [
      "/api/auth/passkeys/addition/options",
      "/api/auth/passkeys/addition/complete",
    ]);
    for (const { init } of requests) {
      equal(init.method, "POST");
      equal(init.credentials, "omit");
      equal(init.mode, "same-origin");
      equal(init.redirect, "error");
      equal(init.cache, "no-store");
      equal(init.referrerPolicy, "no-referrer");
      equal(
        new Headers(init.headers).get("authorization"),
        "Bearer host-test-token",
      );
      equal(String(init.body).includes("host-test-token"), false);
    }
    equal(JSON.parse(String(requests[0].init.body)), {
      browser_origin: "https://yoi.example",
    });
    const complete = JSON.parse(String(requests[1].init.body));
    equal(Object.keys(complete).sort(), ["challenge_id", "credential"]);
    equal(complete.challenge_id, registrationOptions.challenge_id);
    equal(complete.credential.response.attestationObject, "AQID");
  }, async (options) => {
    equal([...new Uint8Array(options.publicKey!.challenge as ArrayBuffer)], [
      1,
      2,
      3,
    ]);
    equal([...new Uint8Array(options.publicKey!.user.id as ArrayBuffer)], [
      4,
      5,
      6,
    ]);
    equal(options.signal === controller.signal, true);
    return credentialFixture();
  });
});

Deno.test("host enrollment refuses missing, oversized and header-injecting tokens before network", async () => {
  let calls = 0;
  await boundary(async () => {
    calls++;
    return Response.json({});
  }, async () => {
    for (const token of ["", " ", "x\r\ny", "x".repeat(4097)]) {
      await rejects(enrollHostingPasskey(token, new AbortController().signal));
    }
    equal(calls, 0);
  });
});

Deno.test("host enrollment sanitizes rejection and thrown diagnostics without retry", async () => {
  for (const failure of [401, 403, 503, "throw"] as const) {
    let calls = 0;
    await boundary(async () => {
      calls++;
      if (failure === "throw") throw new Error("host-test-token in diagnostic");
      return new Response("host-test-token", { status: failure });
    }, async () => {
      await rejects(
        enrollHostingPasskey("host-test-token", new AbortController().signal),
      );
      equal(calls, 1);
    });
  }
});

Deno.test("host enrollment validates options before opening credential prompt", async () => {
  for (
    const payload of [{}, { ...registrationOptions, challenge_id: "" }, {
      ...registrationOptions,
      unexpected: true,
    }]
  ) {
    let calls = 0;
    await boundary(async () => Response.json(payload), async () => {
      await rejects(
        enrollHostingPasskey("host-test-token", new AbortController().signal),
      );
      equal(calls, 0);
    }, async () => {
      calls++;
      return credentialFixture();
    });
  }
});

Deno.test("host enrollment cancellation and teardown prevent completion", async () => {
  for (const abort of [false, true]) {
    const controller = new AbortController();
    let requests = 0;
    await boundary(async () => {
      requests++;
      return Response.json(registrationOptions);
    }, async () => {
      await rejects(enrollHostingPasskey("host-test-token", controller.signal));
      equal(requests, 1);
    }, async () => {
      if (abort) {
        controller.abort();
        return credentialFixture();
      }
      return null;
    });
  }
});

Deno.test("host enrollment cannot confirm a rejected, lost or malformed completion and never retries", async () => {
  for (const mode of ["rejected", "lost", "malformed", "oversized"] as const) {
    let calls = 0;
    await boundary(async (input) => {
      calls++;
      if (String(input).endsWith("/options")) {
        return Response.json(registrationOptions);
      }
      if (mode === "lost") throw new Error("host-test-token");
      if (mode === "rejected") return new Response(null, { status: 401 });
      if (mode === "oversized") {
        return new Response("{}", { headers: { "content-length": "262145" } });
      }
      return Response.json({ user: { handle: "host-test-token" } });
    }, async () => {
      await rejects(
        enrollHostingPasskey("host-test-token", new AbortController().signal),
      );
      equal(calls, 2);
    });
  }
});

Deno.test("host enrollment sends no token on an insecure browser origin", async () => {
  let requests = 0;
  await boundary(async () => {
    requests++;
    return Response.json({});
  }, async () => {
    Object.defineProperty(window, "isSecureContext", { value: false });
    await rejects(
      enrollHostingPasskey("host-test-token", new AbortController().signal),
    );
    equal(requests, 0);
  });
});
