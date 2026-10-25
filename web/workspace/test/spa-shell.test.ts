import { assertEquals } from "jsr:@std/assert";
import { browserSession } from "../src/lib/workspace/auth/session-state.ts";
import { load, ssr } from "../src/routes/+layout.ts";

Deno.test("authentication and hosting shells never require Workspace catalog authority", async () => {
  for (
    const path of [
      "/",
      "/dashboard",
      "/activate",
      "/login",
      "/register",
      "/account",
      "/login/device",
      "/hosting/accounts",
      "/hosting/settings",
      "/hosting/setup",
    ]
  ) {
    const original = globalThis.fetch;
    globalThis.fetch = async () =>
      Response.json({
        actor: {
          user_id: "u",
          account_id: "a",
          handle: "alice",
          display_name: "Alice",
          auth_method: "browser_session",
        },
        authentication_reference: "fixture-authentication",
        session: { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" },
      });
    browserSession.invalidate();
    let calls = 0;
    const event = {
      depends: () => {},
      url: new URL(path, "https://workspace.test"),
      fetch: () => {
        calls++;
        throw new Error("Workspace API must not be called");
      },
    } as unknown as Parameters<typeof load>[0];
    assertEquals(await (await load(event)).deferredCatalog, {
      ok: true,
      data: { accessibleWorkspaces: [], workspaceCatalogError: null },
    });
    assertEquals(calls, 0);
    browserSession.dispose();
    globalThis.fetch = original;
  }
});

Deno.test("workspace frontend serves deep links through the client SPA", () => {
  assertEquals(ssr, false);
});

Deno.test("root admission never starts private catalog while anonymous, expired, loading or unknown", async () => {
  const original = globalThis.fetch;
  try {
    for (
      const condition of ["anonymous", "expired", "error", "loading"] as const
    ) {
      browserSession.invalidate();
      let release!: () => void;
      const pending = new Promise<void>((resolve) => {
        release = resolve;
      });
      globalThis.fetch = async () => {
        if (condition === "loading") await pending;
        return condition === "error" || condition === "loading"
          ? new Response(null, { status: 503 })
          : Response.json({
            actor: null,
            authentication_reference: null,
            session: { kind: condition },
          });
      };
      let privateCalls = 0;
      const event = {
        url: new URL("https://workspace.test/w/team/tickets?q=mine#entry"),
        depends: () => {},
        fetch: () => {
          ++privateCalls;
          throw new Error("private fetch before admission");
        },
      } as unknown as Parameters<typeof load>[0];
      const result = load(event).then(() => null, (rejection) => rejection);
      await Promise.resolve();
      assertEquals(privateCalls, 0);
      release();
      const rejected = await result;
      assertEquals(
        rejected.status,
        condition === "anonymous" || condition === "expired" ? 307 : 503,
      );
      if (condition === "anonymous") assertEquals(rejected.location, "/");
      if (condition === "expired") {
        assertEquals(
          rejected.location,
          "/login?returnTo=%2Fw%2Fteam%2Ftickets%3Fq%3Dmine%23entry",
        );
      }
      assertEquals(privateCalls, 0);
    }
  } finally {
    browserSession.dispose();
    globalThis.fetch = original;
  }
});
