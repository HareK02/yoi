// @vitest-environment happy-dom
import { cleanup, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import { createRawSnippet } from "svelte";
import SessionBoundary from "./SessionBoundary.svelte";
import { browserSession } from "./session-state.ts";
import { authUser, whoamiPayload } from "./api.test-fixtures.ts";
import { goto } from "$app/navigation";
const page = vi.hoisted(() => ({
  url: new URL("http://localhost:3000/w/team/tickets?q=mine#entry"),
}));
vi.mock("$app/state", () => ({ page }));
vi.mock(
  "$app/navigation",
  () => ({ goto: vi.fn(async () => {}), invalidate: vi.fn(async () => {}) }),
);
afterEach(() => {
  cleanup();
  browserSession.invalidate();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  vi.clearAllMocks();
});
const actor = { ...authUser, auth_method: "browser_session" };
const children = createRawSnippet(() => ({
  render: () => "<p>Private fragment</p>",
}));
async function admitted() {
  window.history.replaceState({}, "", "/w/team/tickets?q=mine#entry");
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => Response.json(whoamiPayload(actor))),
  );
  await browserSession.refresh();
  render(SessionBoundary, { children });
  await screen.findByText("Private fragment");
}

test("active expired state removes private content and preserves browser query and fragment", async () => {
  await admitted();
  vi.stubGlobal(
    "fetch",
    vi.fn(async () =>
      Response.json({ actor: null, session: { kind: "expired" } })
    ),
  );
  await browserSession.refresh();
  await waitFor(() =>
    expect(screen.queryByText("Private fragment")).toBeNull()
  );
  expect(goto).toHaveBeenCalledWith(
    "/login?returnTo=%2Fw%2Fteam%2Ftickets%3Fq%3Dmine%23entry",
    { replaceState: true },
  );
});

test("unknown state hides and inerts admitted content instead of declaring anonymous", async () => {
  await admitted();
  let release!: (response: Response) => void;
  vi.stubGlobal(
    "fetch",
    vi.fn(() =>
      new Promise<Response>((resolve) => {
        release = resolve;
      })
    ),
  );
  const checking = browserSession.refresh();
  await screen.findByText("Loading session…");
  expect(screen.getByText("Private fragment").closest("[hidden][inert]"))
    .toBeTruthy();
  release(new Response(null, { status: 503 }));
  await checking;
  await screen.findByText(
    "Your session could not be loaded. Login status is unknown.",
  );
  expect(screen.getByText("Private fragment").closest("[hidden][inert]"))
    .toBeTruthy();
  expect(goto).not.toHaveBeenCalled();
});

test("another tab changing actor cannot reveal content loaded for the old actor", async () => {
  await admitted();
  const reload = vi.spyOn(location, "reload").mockImplementation(() => {});
  vi.stubGlobal(
    "fetch",
    vi.fn(async () =>
      Response.json(
        whoamiPayload({
          ...actor,
          user_id: "different-user",
          account_id: "different-account",
        }),
      )
    ),
  );
  await browserSession.refresh();
  await waitFor(() => expect(reload).toHaveBeenCalledOnce());
  expect(screen.queryByText("Private fragment")).toBeNull();
});

test("a child mount session recheck does not unmount and restart its own loader", async () => {
  window.history.replaceState({}, "", "/w/team/tickets?q=mine#entry");
  const fetcher = vi.fn(async () => Response.json(whoamiPayload(actor)));
  vi.stubGlobal("fetch", fetcher);
  await browserSession.refresh();
  let mounts = 0;
  const checkingChild = createRawSnippet(() => ({
    render: () => "<p>Mounted session reader</p>",
    setup: () => {
      ++mounts;
      void browserSession.refresh();
    },
  }));
  render(SessionBoundary, { children: checkingChild });
  await waitFor(() => expect(browserSession.snapshot.kind).toBe("ready"));
  expect(screen.getByText("Mounted session reader").closest("[hidden]"))
    .toBeNull();
  expect(mounts).toBe(1);
  expect(fetcher).toHaveBeenCalledTimes(2);
});
