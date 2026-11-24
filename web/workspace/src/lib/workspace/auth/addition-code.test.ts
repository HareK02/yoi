declare const Deno: { test(name: string, fn: () => Promise<void>): void };
import {
  AdditionCodeError,
  issueAdditionCode,
  loadAdditionCodeStatus,
  redeemAdditionCode,
  revokeAdditionCode,
} from "./addition-code.ts";
import {
  authUser,
  credentialFixture,
  registrationOptions,
  whoamiPayload,
} from "./api.test-fixtures.ts";
const base = "/api/auth/passkeys/addition-code";
const target = "https://example.test";
const meta = {
  issuance_id: "issuance-test",
  target_origin: target,
  expires_at: "2099-01-01T00:00:00Z",
};
const issued = { user: authUser, ...meta, code: "synthetic-secret-only" };
const whoami = () =>
  whoamiPayload({ ...authUser, auth_method: "browser_session" });
function equal(a: unknown, b: unknown) {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error("Values differ");
}
async function reject(
  promise: Promise<unknown>,
  kind: AdditionCodeError["kind"],
) {
  try {
    await promise;
  } catch (e) {
    equal(e instanceof AdditionCodeError, true);
    equal((e as AdditionCodeError).kind, kind);
    equal((e as Error).message.includes(issued.code), false);
    return;
  }
  throw new Error("Expected rejection");
}
async function boundary(
  fetcher: typeof fetch,
  run: () => Promise<void>,
  create = async (): Promise<Credential | null> => credentialFixture(),
) {
  const saved = ["fetch", "window", "navigator"].map((key) =>
    [key, Object.getOwnPropertyDescriptor(globalThis, key)] as const
  );
  Object.defineProperties(globalThis, {
    fetch: { configurable: true, value: fetcher },
    window: {
      configurable: true,
      value: { isSecureContext: true, location: { origin: target } },
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
Deno.test("addition code owner mutations bind live identity, use dedicated paths and never send secrets as auth", async () => {
  for (const action of ["issue", "revoke", "status"] as const) {
    const calls: Array<{ path: string; init: RequestInit }> = [];
    await boundary(async (input, init) => {
      const path = String(input);
      calls.push({ path, init: init! });
      return Response.json(
        path.endsWith("whoami")
          ? whoami()
          : action === "issue"
          ? issued
          : action === "revoke"
          ? { revoked: true }
          : { user: authUser, public_urls: [target], active: meta },
      );
    }, async () => {
      const signal = new AbortController().signal;
      if (action === "issue") {
        equal(await issueAdditionCode(authUser, target, signal), issued);
      } else if (action === "revoke") {
        equal(
          await revokeAdditionCode(authUser, meta.issuance_id, signal),
          true,
        );
      } else {equal(
          (await loadAdditionCodeStatus(authUser, signal)).active,
          meta,
        );}
      equal(calls.map((c) => c.path), [
        "/api/auth/whoami",
        action === "revoke" ? `${base}/revoke` : base,
        "/api/auth/whoami",
      ]);
      const init = calls[1].init;
      equal(init.credentials, "same-origin");
      equal(init.cache, "no-store");
      equal(init.redirect, "error");
      equal(init.referrerPolicy, "no-referrer");
      equal(new Headers(init.headers).has("authorization"), false);
      if (action !== "status") {
        const body = JSON.parse(String(init.body));
        equal(body.expected_user_id, authUser.user_id);
        equal(body.expected_authentication_reference, "fixture-authentication");
        equal(
          action === "revoke" ? body.issuance_id : body.target_origin,
          action === "revoke" ? meta.issuance_id : target,
        );
      }
    });
  }
});
Deno.test("addition code issuance with changed owner or session never exposes returned code", async () => {
  for (
    const mode of ["before", "after", "response", "target", "aborted"] as const
  ) {
    let whoamis = 0;
    let mutations = 0;
    const controller = new AbortController();
    await boundary(async (input) => {
      if (String(input).endsWith("whoami")) {
        whoamis++;
        return Response.json(
          (mode === "before" || mode === "after" && whoamis === 2)
            ? whoamiPayload({
              ...authUser,
              account_id: "other",
              auth_method: "browser_session",
            })
            : whoami(),
        );
      }
      mutations++;
      if (mode === "aborted") controller.abort();
      return Response.json(
        mode === "response"
          ? { ...issued, user: { ...authUser, account_id: "other" } }
          : mode === "target"
          ? { ...issued, target_origin: "https://elsewhere.test" }
          : issued,
      );
    }, async () => {
      await reject(
        issueAdditionCode(authUser, target, controller.signal),
        mode === "before" || mode === "after"
          ? "session-changed"
          : "unconfirmed",
      );
      equal(mutations, mode === "before" ? 0 : 1);
    });
  }
});
Deno.test("addition code issuance distinguishes recent-auth denial and unknown outcome without retry or diagnostic leakage", async () => {
  for (const mode of ["fresh", "network", "server"] as const) {
    let calls = 0;
    await boundary(async (input) => {
      if (String(input).endsWith("whoami")) return Response.json(whoami());
      calls++;
      if (mode === "network") throw new Error(issued.code);
      return Response.json({
        message: issued.code,
        diagnostics: [{
          code: mode === "fresh"
            ? "passkey_recent_authentication_required"
            : "other",
          message: issued.code,
        }],
      }, { status: mode === "fresh" ? 403 : 503 });
    }, async () => {
      await reject(
        issueAdditionCode(authUser, target, new AbortController().signal),
        mode === "fresh" ? "recent-auth" : "unconfirmed",
      );
      equal(calls, 1);
    });
  }
});
Deno.test("code redemption binds destination and account, sends code only to options, confirms session after WebAuthn", async () => {
  const calls: Array<{ path: string; init: RequestInit }> = [];
  await boundary(async (input, init) => {
    const path = String(input);
    calls.push({ path, init: init! });
    return Response.json(
      path.endsWith("whoami")
        ? whoami()
        : path.endsWith("/options")
        ? registrationOptions
        : {
          user: authUser,
          authentication_reference: "fixture-authentication",
        },
    );
  }, async () => {
    equal(
      await redeemAdditionCode(
        " ALIce ",
        issued.code,
        new AbortController().signal,
      ),
      authUser,
    );
    equal(calls.map((c) => c.path), [
      `${base}/options`,
      `${base}/complete`,
      "/api/auth/whoami",
    ]);
    equal(JSON.parse(String(calls[0].init.body)), {
      handle: "alice",
      code: issued.code,
      browser_origin: target,
    });
    equal(String(calls[1].init.body).includes(issued.code), false);
    equal(calls.some((c) => c.path.includes(issued.code)), false);
  });
});
Deno.test("cancelled code WebAuthn never sends completion and lost completion is unknown without retry", async () => {
  for (const mode of ["cancel", "lost", "wrong-owner"] as const) {
    let completions = 0;
    await boundary(async (input) => {
      if (String(input).endsWith("/options")) {
        return Response.json(registrationOptions);
      }
      completions++;
      if (mode === "lost") throw new Error(issued.code);
      return Response.json({
        user: { ...authUser, handle: "other" },
        authentication_reference: "fixture-authentication",
      });
    }, async () => {
      await reject(
        redeemAdditionCode("alice", issued.code, new AbortController().signal),
        mode === "cancel" ? "rejected" : "unconfirmed",
      );
      equal(completions, mode === "cancel" ? 0 : 1);
    }, async () => mode === "cancel" ? null : credentialFixture());
  }
});
