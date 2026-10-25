declare const Deno: { test(name: string, fn: () => Promise<void>): void };
import { browserSession } from "./session-state.ts";
import { activateWithPasskey } from "./activation.ts";
import {
  authUser,
  credentialFixture,
  registrationOptions,
} from "./api.test-fixtures.ts";
function confirmedSession() {
  return {
    actor: { ...authUser, auth_method: "browser_session" },
    authentication_reference: "fixture-authentication",
    session: { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" },
  };
}
function equal(a: unknown, b: unknown) {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error("Values differ");
}
async function rejects(run: Promise<unknown>) {
  try {
    await run;
  } catch (error) {
    equal((error as Error).message, "Activation could not be confirmed.");
    return;
  }
  throw new Error("Expected rejection");
}
async function boundary(
  fetcher: typeof fetch,
  create: (options: CredentialCreationOptions) => Promise<Credential | null>,
  run: () => Promise<void>,
) {
  const saved = ["fetch", "window", "navigator"].map((key) =>
    [key, Object.getOwnPropertyDescriptor(globalThis, key)] as const
  );
  Object.defineProperties(globalThis, {
    fetch: { configurable: true, value: fetcher },
    window: {
      configurable: true,
      value: {
        isSecureContext: true,
        location: { origin: "https://example.test" },
      },
    },
    navigator: { configurable: true, value: { credentials: { create } } },
  });
  try {
    await run();
  } finally {
    for (const [key, descriptor] of saved) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  }
}
// HTTP/WebAuthn境界の代替。本物のServerによるcode検証やcredential保存の試験ではない。
Deno.test("activation uses only dedicated endpoints and confirms normalized target after completion", async () => {
  const requests: { path: string; init: RequestInit }[] = [];
  const signal = new AbortController().signal;
  await boundary(async (input, init) => {
    requests.push({ path: String(input), init: init! });
    return Response.json(
      String(input) === "/api/auth/whoami"
        ? confirmedSession()
        : String(input).endsWith("/options")
        ? registrationOptions
        : {
          user: authUser,
          authentication_reference: "fixture-authentication",
        },
    );
  }, async (options) => {
    equal(options.signal === signal, true);
    return credentialFixture();
  }, async () => {
    equal(
      await activateWithPasskey(" ALIce ", "synthetic-input-only", signal),
      authUser,
    );
    equal(requests.map((r) => r.path), [
      "/api/auth/activation/options",
      "/api/auth/activation/complete",
      "/api/auth/whoami",
    ]);
    for (const { path, init } of requests) {
      equal(init.method ?? "GET", path === "/api/auth/whoami" ? "GET" : "POST");
      equal(init.signal === signal, true);
      equal(init.credentials, "same-origin");
      equal(init.redirect, "error");
      equal(init.mode, "same-origin");
      equal(init.cache, "no-store");
      if (path !== "/api/auth/whoami") {
        equal(init.referrerPolicy, "no-referrer");
      }
      equal(new Headers(init.headers).has("authorization"), false);
    }
    equal(requests[2].init.body, undefined);
    equal(JSON.parse(String(requests[0].init.body)), {
      handle: "alice",
      code: "synthetic-input-only",
      browser_origin: "https://example.test",
    });
    equal(Object.keys(JSON.parse(String(requests[1].init.body))).sort(), [
      "challenge_id",
      "credential",
    ]);
    equal(
      String(requests[1].init.body).includes("synthetic-input-only"),
      false,
    );
  });
});
Deno.test("activation cancellation and abort never send complete or return active", async () => {
  for (const mode of ["cancel", "abort", "null"] as const) {
    let calls = 0;
    const controller = new AbortController();
    await boundary(async () => {
      calls++;
      return Response.json(registrationOptions);
    }, async () => {
      if (mode === "cancel") throw new Error("synthetic-input-only");
      if (mode === "abort") controller.abort();
      return mode === "null" ? null : credentialFixture();
    }, async () => {
      await rejects(
        activateWithPasskey("alice", "synthetic-input-only", controller.signal),
      );
      equal(calls, 1);
    });
  }
});
Deno.test("activation sanitizes HTTP transport malformed and mismatched completion without retry", async () => {
  for (const mode of ["http", "throw", "malformed", "mismatch"] as const) {
    let calls = 0;
    await boundary(
      async (input) => {
        calls++;
        if (mode === "throw") throw new Error("synthetic-input-only");
        if (mode === "http") {
          return new Response("synthetic-input-only", { status: 403 });
        }
        if (mode === "malformed") {
          return Response.json({ unexpected: "synthetic-input-only" });
        }
        return Response.json(
          String(input).endsWith("/options") ? registrationOptions : {
            user: { ...authUser, handle: "other" },
            authentication_reference: "fixture-authentication",
          },
        );
      },
      async () => credentialFixture(),
      async () => {
        await rejects(
          activateWithPasskey(
            "alice",
            "synthetic-input-only",
            new AbortController().signal,
          ),
        );
        equal(calls, mode === "mismatch" ? 2 : 1);
      },
    );
  }
});
Deno.test("activation invalid IDs and oversized code are rejected before network", async () => {
  let calls = 0;
  await boundary(
    async () => {
      calls++;
      return Response.json({});
    },
    async () => credentialFixture(),
    async () => {
      for (const handle of ["", "a/b", "Ａ", "i̇", "a".repeat(65)]) {
        await rejects(
          activateWithPasskey(
            handle,
            "synthetic-input-only",
            new AbortController().signal,
          ),
        );
      }
      await rejects(
        activateWithPasskey(
          "alice",
          "x".repeat(4097),
          new AbortController().signal,
        ),
      );
      equal(calls, 0);
    },
  );
});

for (
  const mode of [
    "anonymous",
    "bob",
    "account-mismatch",
    "reference-mismatch",
    "api-token",
    "unavailable",
  ] as const
) {
  Deno.test(
    "activation rejects " + mode +
      " whoami after successful completion without retry",
    async () => {
      const paths: string[] = [];
      await boundary(
        async (input) => {
          const path = String(input);
          paths.push(path);
          if (path === "/api/auth/activation/options") {
            return Response.json(registrationOptions);
          }
          if (path === "/api/auth/activation/complete") {
            return Response.json({
              user: authUser,
              authentication_reference: "fixture-authentication",
            });
          }
          if (path !== "/api/auth/whoami") {
            throw new Error("Unexpected endpoint");
          }
          if (mode === "unavailable") {
            return new Response("private whoami diagnostic", { status: 503 });
          }
          if (mode === "anonymous") {
            return Response.json({
              actor: null,
              authentication_reference: null,
              session: { kind: "anonymous" },
            });
          }
          const confirmation = confirmedSession();
          if (mode === "bob") {
            confirmation.actor = {
              ...confirmation.actor,
              user_id: "user-bob",
              account_id: "account-bob",
              handle: "bob",
              display_name: "Bob",
            };
          }
          if (mode === "account-mismatch") {
            confirmation.actor.account_id = "account-bob";
          }
          if (mode === "reference-mismatch") {
            confirmation.authentication_reference = "another-authentication";
          }
          if (mode === "api-token") {
            confirmation.actor.auth_method = "api_token";
          }
          return Response.json(confirmation);
        },
        async () => credentialFixture(),
        async () => {
          await rejects(
            activateWithPasskey(
              "alice",
              "synthetic-input-only",
              new AbortController().signal,
            ),
          );
          equal(paths, [
            "/api/auth/activation/options",
            "/api/auth/activation/complete",
            "/api/auth/whoami",
          ]);
        },
      );
    },
  );
}

for (const stage of ["prompt", "completion", "confirmation"] as const) {
  Deno.test(
    "activation session generation change during " + stage +
      " fences stale work",
    async () => {
      const paths: string[] = [];
      await boundary(async (input) => {
        const path = String(input);
        paths.push(path);
        if (path === "/api/auth/activation/options") {
          return Response.json(registrationOptions);
        }
        if (path === "/api/auth/activation/complete") {
          if (stage === "completion") browserSession.invalidate();
          return Response.json({
            user: authUser,
            authentication_reference: "fixture-authentication",
          });
        }
        if (path === "/api/auth/whoami") {
          if (stage === "confirmation") browserSession.invalidate();
          return Response.json(confirmedSession());
        }
        throw new Error("Unexpected endpoint");
      }, async () => {
        if (stage === "prompt") browserSession.invalidate();
        return credentialFixture();
      }, async () => {
        await rejects(
          activateWithPasskey(
            "alice",
            "synthetic-input-only",
            new AbortController().signal,
          ),
        );
        equal(paths, [
          "/api/auth/activation/options",
          ...(stage === "prompt" ? [] : ["/api/auth/activation/complete"]),
          ...(stage === "confirmation" ? ["/api/auth/whoami"] : []),
        ]);
      });
    },
  );
}
