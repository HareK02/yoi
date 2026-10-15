declare const Deno: { test(name: string, fn: () => Promise<void>): void };
import {
  issueActivationCode,
  normalizeActivationHandle,
} from "./hosting-accounts.ts";
import { HostingRequestError } from "./hosting.ts";
import { authUser } from "./api.test-fixtures.ts";
function equal(a: unknown, b: unknown) {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error("Values differ");
}
async function boundary(fetcher: typeof fetch, run: () => Promise<void>) {
  const saved = globalThis.fetch;
  globalThis.fetch = fetcher;
  try {
    await run();
  } finally {
    globalThis.fetch = saved;
  }
}
const response = () => ({
  user: authUser,
  code: "synthetic-output-only",
  expires_at: new Date(Date.now() + 900000).toISOString(),
});
Deno.test("hosting issuance normalizes ASCII handle and selects create or explicit reissue using cookie only", async () => {
  for (const reissue of [false, true]) {
    let calls = 0;
    await boundary(async (input, requestInit) => {
      const init = requestInit as RequestInit;
      calls++;
      equal(
        input,
        reissue
          ? "/api/hosting/accounts/activation-code"
          : "/api/hosting/accounts",
      );
      equal(JSON.parse(String(init?.body)), { handle: "alice" });
      equal(init?.method, "POST");
      equal(init?.credentials, "same-origin");
      equal(init?.redirect, "error");
      equal(init?.cache, "no-store");
      equal(init?.referrerPolicy, "no-referrer");
      equal(new Headers(init?.headers).has("authorization"), false);
      return Response.json(response());
    }, async () => {
      const issued = await issueActivationCode(
        " ALIce ",
        reissue,
        new AbortController().signal,
      );
      equal(issued.user, authUser);
      equal(calls, 1);
    });
  }
});
Deno.test("hosting issuance rejects malformed mismatched secret-bearing responses and transport diagnostics", async () => {
  for (
    const mode of ["throw", "malformed", "mismatch", "extra", "expiry"] as const
  ) {
    await boundary(async () => {
      if (mode === "throw") throw new Error("synthetic-output-only");
      const value = response();
      return Response.json(
        mode === "malformed"
          ? { code: value.code }
          : mode === "extra"
          ? { ...value, extra: true }
          : mode === "expiry"
          ? { ...value, expires_at: "never" }
          : { ...value, user: { ...authUser, handle: "other" } },
      );
    }, async () => {
      try {
        await issueActivationCode("alice", false, new AbortController().signal);
      } catch (error) {
        equal(
          (error as Error).message,
          "Code issuance could not be confirmed.",
        );
        return;
      }
      throw new Error("Expected rejection");
    });
  }
});
Deno.test("hosting issuance exposes only HTTP status for denial and never retries", async () => {
  for (const status of [401, 403, 409, 503]) {
    let calls = 0;
    await boundary(async () => {
      calls++;
      return new Response("synthetic-output-only", { status });
    }, async () => {
      try {
        await issueActivationCode("alice", true, new AbortController().signal);
      } catch (error) {
        equal(error instanceof HostingRequestError, true);
        equal((error as HostingRequestError).status, status);
        equal(
          (error as Error).message.includes("synthetic-output-only"),
          false,
        );
        equal(calls, 1);
        return;
      }
      throw new Error("Expected rejection");
    });
  }
});
Deno.test("hosting ID normalization rejects Unicode and invalid length instead of changing account identity", async () => {
  equal(normalizeActivationHandle(" AbC-_09 "), "abc-_09");
  for (const value of ["", "a".repeat(65), "Ａ", "a.b", "a b", "İ"]) {
    let rejected = false;
    try {
      normalizeActivationHandle(value);
    } catch {
      rejected = true;
    }
    equal(rejected, true);
  }
});
