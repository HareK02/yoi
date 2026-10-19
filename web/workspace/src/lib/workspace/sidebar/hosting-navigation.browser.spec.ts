// @vitest-environment happy-dom
import { cleanup, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import GlobalNavSections from "./GlobalNavSections.svelte";
import { authUser } from "#lib/workspace/auth/api.test-fixtures.ts";
import { notifySessionChanged } from "#lib/workspace/auth/hosting.ts";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

for (
  const persona of ["anonymous", "ordinary", "admin", "unavailable"] as const
) {
  test(`global navigation exposes hosting settings only for confirmed admin, not ${persona}`, async () => {
    let actor = persona !== "anonymous";
    const fetcher = vi.fn<typeof fetch>().mockImplementation(async (input) => {
      if (String(input) === "/api/auth/whoami") {
        return Response.json({
          session: actor
            ? { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" }
            : { kind: "anonymous" },
          actor: actor ? { ...authUser, auth_method: "browser_session" } : null,
        });
      }
      if (String(input) === "/api/hosting/access") {
        if (persona === "unavailable") {
          return new Response(null, { status: 503 });
        }
        return Response.json({ is_admin: persona === "admin" });
      }
      throw new Error(`Unexpected API ${input}`);
    });
    vi.stubGlobal("fetch", fetcher);
    render(GlobalNavSections, {
      currentPath: "/hosting/settings",
      workspaces: [],
    });
    await waitFor(() => expect(fetcher).toHaveBeenCalled());
    if (persona === "admin") {
      const link = await screen.findByRole("link", {
        name: "Hosting settings",
      });
      expect(link.getAttribute("href")).toBe("/hosting/settings");
      expect(link.getAttribute("aria-current")).toBe("page");
      actor = false;
      notifySessionChanged();
      await waitFor(() =>
        expect(screen.queryByRole("link", { name: "Hosting settings" }))
          .toBeNull()
      );
    } else {
      if (persona !== "anonymous") {
        await waitFor(() =>
          expect(
            fetcher.mock.calls.some(([path]) => path === "/api/hosting/access"),
          ).toBe(true)
        );
      }
      expect(screen.queryByRole("link", { name: "Hosting settings" }))
        .toBeNull();
    }
  });
}
