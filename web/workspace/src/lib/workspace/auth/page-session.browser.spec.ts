// @vitest-environment happy-dom
import { afterEach, expect, test, vi } from "vitest";
import { browserSession, observeBrowserSession } from "./session-state.ts";
import { requirePageSession } from "./page-session.ts";
import { loadJson } from "../api/http.ts";
import { authUser, whoamiPayload } from "./api.test-fixtures.ts";

const actor = { ...authUser, auth_method: "browser_session" };
const target = new URL("http://localhost/w/team/tickets?q=mine#entry");
afterEach(() => {
  browserSession.invalidate();
  localStorage.clear();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

function fixture() {
  const fetcher = vi.fn(async () => Response.json(whoamiPayload(actor)));
  vi.stubGlobal("fetch", fetcher);
  return fetcher;
}

test("focus, visibility and pageshow never recheck an admitted session", async () => {
  const fetcher = fixture();
  await requirePageSession(target);
  const admitted = browserSession.snapshot;
  for (let i = 0; i < 3; i++) {
    window.dispatchEvent(new Event("focus"));
    window.dispatchEvent(new Event("pageshow"));
    document.dispatchEvent(new Event("visibilitychange"));
  }
  await Promise.resolve();
  expect(fetcher).toHaveBeenCalledTimes(1);
  expect(browserSession.snapshot).toBe(admitted);
});

test("concurrent API 401 responses return errors without checking or changing the session", async () => {
  const fetcher = fixture();
  await requirePageSession(target);
  const admitted = browserSession.snapshot;
  const unauthorized = vi.fn(async () =>
    new Response("denied", { status: 401 })
  );
  const results = await Promise.all([
    loadJson(unauthorized, "/api/w/team/workspace"),
    loadJson(unauthorized, "/api/w/team/repositories"),
  ]);
  for (const result of results) {
    expect(result).toEqual({ data: null, error: "denied", status: 401 });
  }
  expect(fetcher).toHaveBeenCalledTimes(1);
  expect(browserSession.snapshot).toBe(admitted);
});

for (const kind of ["anonymous", "expired"] as const) {
  test(`the next protected page access checks again and redirects ${kind}`, async () => {
    const fetcher = fixture();
    await requirePageSession(target);
    await browserSession.ensure();
    expect(fetcher).toHaveBeenCalledTimes(1);
    fetcher.mockResolvedValueOnce(
      Response.json({ actor: null, session: { kind } }),
    );
    await expect(requirePageSession(target)).rejects.toMatchObject({
      status: 307,
      location: kind === "anonymous"
        ? "/"
        : "/login?returnTo=%2Fw%2Fteam%2Ftickets%3Fq%3Dmine%23entry",
    });
    expect(fetcher).toHaveBeenCalledTimes(2);
  });
}

test("storage notifications fence the cache without background requests or loading", async () => {
  const fetcher = fixture();
  observeBrowserSession();
  await requirePageSession(target);
  const admitted = browserSession.snapshot;
  const key = "yoi.browser-auth-generation";
  const newValue = JSON.stringify({
    id: "other-tab",
    reason: "authentication",
  });
  localStorage.setItem(key, newValue);
  window.dispatchEvent(new StorageEvent("storage", { key, newValue }));
  await Promise.resolve();
  expect(fetcher).toHaveBeenCalledTimes(1);
  expect(browserSession.snapshot).toBe(admitted);
  await requirePageSession(target);
  expect(fetcher).toHaveBeenCalledTimes(2);
});

test("a failed page check reports unknown instead of admitting or redirecting", async () => {
  const fetcher = fixture();
  await requirePageSession(target);
  fetcher.mockResolvedValueOnce(new Response(null, { status: 503 }));
  await expect(requirePageSession(target)).rejects.toMatchObject({
    status: 503,
  });
  expect(browserSession.snapshot.kind).toBe("error");
});
