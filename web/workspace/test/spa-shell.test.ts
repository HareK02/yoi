import { assertEquals } from "jsr:@std/assert";
import { load, ssr } from "../src/routes/+layout.ts";

Deno.test("authentication and hosting shells never require Workspace catalog authority", async () => {
  for (
    const path of [
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
    let calls = 0;
    const event = {
      url: new URL(path, "https://workspace.test"),
      fetch: () => {
        calls++;
        throw new Error("Workspace API must not be called");
      },
    } as unknown as Parameters<typeof load>[0];
    assertEquals(await load(event).deferredCatalog, {
      ok: true,
      data: { accessibleWorkspaces: [], workspaceCatalogError: null },
    });
    assertEquals(calls, 0);
  }
});

Deno.test("workspace frontend serves deep links through the client SPA", () => {
  assertEquals(ssr, false);
});
