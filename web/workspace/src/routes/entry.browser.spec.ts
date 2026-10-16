// @vitest-environment happy-dom
// UI→本番clientを接続しHTTPだけ代替する。実Serverの認証・認可・SQLiteは共有E2Eが所有する。
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import Home from "./+page.svelte";
import Dashboard from "./dashboard/+page.svelte";
import { authUser } from "#lib/workspace/auth/api.test-fixtures.ts";
import {
  createdWorkspace,
  creationResponse,
} from "#lib/workspace/api/workspace-catalog.test-fixtures.ts";
import { SESSION_CHANGED_EVENT } from "#lib/workspace/auth/hosting.ts";
import { goto } from "$app/navigation";
vi.mock("$app/navigation", () => ({ goto: vi.fn(async () => {}) }));
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}
function boundary() {
  const state = {
    signedIn: false,
    whoamiStatus: 200,
    policy: true as boolean | "error" | "loading",
    admin: false,
    catalogStatus: 200,
    count: 0,
    createStatus: 200,
    sessionGate: null as ReturnType<typeof deferred<Response>> | null,
    catalogGate: null as ReturnType<typeof deferred<Response>> | null,
  };
  const fetcher = vi.fn<typeof fetch>().mockImplementation(
    async (input, init) => {
      const path = String(input);
      if (path === "/api/auth/whoami") {
        if (state.sessionGate) return state.sessionGate.promise;
        return state.whoamiStatus === 200
          ? Response.json({
            actor: state.signedIn
              ? { ...authUser, auth_method: "browser_session" }
              : null,
          })
          : new Response(null, { status: state.whoamiStatus });
      }
      if (path === "/api/auth/registration-policy") {
        if (state.policy === "loading") return new Promise<Response>(() => {});
        return state.policy === "error"
          ? new Response(null, { status: 503 })
          : Response.json({ self_registration_enabled: state.policy });
      }
      if (path === "/api/hosting/access") {
        return Response.json({ is_admin: state.admin });
      }
      if (init?.method === "POST") {
        return state.createStatus === 200
          ? Response.json(creationResponse())
          : new Response(null, { status: state.createStatus });
      }
      if (path.startsWith("/api/workspaces?")) {
        if (state.catalogGate) return state.catalogGate.promise;
        return state.catalogStatus === 200
          ? Response.json(
            Array.from(
              { length: state.count },
              (_, i) => ({
                ...createdWorkspace,
                workspace_id: `space-${i}`,
                display_name: `Team ${i}`,
              }),
            ),
          )
          : new Response(null, { status: state.catalogStatus });
      }
      if (path.includes("/repositories")) {
        return Response.json({
          workspace_id: "fixture",
          items: [],
          source: "workspace-control-plane",
          diagnostics: [],
        });
      }
      throw new Error(`Unexpected request ${path}`);
    },
  );
  vi.stubGlobal("fetch", fetcher);
  return {
    state,
    fetcher,
    workspaceCalls: () =>
      fetcher.mock.calls.filter(([path]) =>
        String(path).startsWith("/api/workspaces")
      ),
  };
}
for (const policy of [true, false, "error", "loading"] as const) {
  test(`public home keeps login and exposes registration only for confirmed allowed policy: ${policy}`, async () => {
    const { state, workspaceCalls } = boundary();
    state.policy = policy;
    state.catalogStatus = 401;
    render(Home);
    expect(
      (await screen.findByRole("link", { name: "Log in" })).getAttribute(
        "href",
      ),
    ).toBe("/login");
    if (policy === true) {
      expect(
        (await screen.findByRole("link", { name: "Create a new account" }))
          .getAttribute("href"),
      ).toBe("/register?returnTo=%2Fdashboard");
    } else {
      await screen.findByText(
        policy === false
          ? /registration is disabled/
          : policy === "error"
          ? /availability is unknown/
          : /Checking registration/,
      );
      expect(screen.queryByRole("link", { name: "Create a new account" }))
        .toBeNull();
    }
    expect(workspaceCalls()).toEqual([]);
    expect(screen.queryByText(/No accessible Workspaces/)).toBeNull();
  });
}
test("whoami loading and failure never expose anonymous actions or Workspace content; retry confirms actor", async () => {
  const { state, workspaceCalls } = boundary();
  state.sessionGate = deferred<Response>();
  render(Home);
  await screen.findByText("Loading session…");
  expect(screen.queryByRole("link")).toBeNull();
  state.sessionGate.resolve(new Response(null, { status: 503 }));
  await screen.findByRole("alert");
  expect(screen.queryByRole("link", { name: "Log in" })).toBeNull();
  expect(screen.queryByRole("link", { name: "Create a new account" }))
    .toBeNull();
  state.sessionGate = null;
  state.signedIn = true;
  await fireEvent.click(screen.getByRole("button", { name: "Retry session" }));
  expect(
    (await screen.findByRole("link", { name: "Open dashboard" })).getAttribute(
      "href",
    ),
  ).toBe("/dashboard");
  expect(workspaceCalls()).toEqual([]);
  expect(goto).not.toHaveBeenCalled();
});
for (const admin of [false, true]) {
  test(`confirmed actor account/hosting navigation reflects live admin=${admin}`, async () => {
    const { state } = boundary();
    state.signedIn = true;
    state.admin = admin;
    render(Home);
    await screen.findByRole("link", { name: "Account" });
    if (admin) await screen.findByRole("link", { name: "Hosting settings" });
    else {expect(screen.queryByRole("link", { name: "Hosting settings" }))
        .toBeNull();}
    state.signedIn = false;
    await fireEvent(window, new Event(SESSION_CHANGED_EVENT));
    await screen.findByRole("link", { name: "Log in" });
    expect(screen.queryByRole("link", { name: "Hosting settings" })).toBeNull();
  });
}
for (const count of [0, 1, 3]) {
  test(`dashboard ${count} Workspaces requires explicit selection and offers creation`, async () => {
    const { state } = boundary();
    state.signedIn = true;
    state.count = count;
    render(Dashboard);
    if (count === 0) await screen.findByText("No accessible Workspaces");
    else {
      await screen.findByRole("link", { name: /Team 0/ });
      expect(screen.getAllByRole("link", { name: /Team \d/ })).toHaveLength(
        count,
      );
      expect(screen.getByRole("link", { name: /Team 0/ }).getAttribute("href"))
        .toBe("/w/space-0");
    }
    expect(goto).not.toHaveBeenCalled();
    await fireEvent.click(
      screen.getByText("Create Workspace", { selector: "summary" }),
    );
    await fireEvent.input(screen.getByLabelText("Workspace display name"), {
      target: { value: "New Workspace" },
    });
    await fireEvent.submit(
      screen.getByRole("button", { name: "Create Workspace" }).closest("form")!,
    );
    await waitFor(() => expect(goto).toHaveBeenCalledWith("/w/home-empty"));
  });
}
for (const status of [401, 503]) {
  test(`dashboard catalog failure ${status} is not empty; account and creation recovery remain`, async () => {
    const { state } = boundary();
    state.signedIn = true;
    state.catalogStatus = status;
    render(Dashboard);
    await screen.findByRole("alert");
    expect(screen.queryByText("No accessible Workspaces")).toBeNull();
    expect(screen.getByRole("link", { name: "Account" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Refresh" })).toBeTruthy();
  });
}
test("dashboard removes private content on session change and ignores old catalog completion", async () => {
  const { state } = boundary();
  state.signedIn = true;
  state.catalogGate = deferred<Response>();
  render(Dashboard);
  await screen.findByText("Loading Workspaces…");
  state.signedIn = false;
  await fireEvent(window, new Event(SESSION_CHANGED_EVENT));
  await screen.findByText("Log in to select or create a Workspace.");
  state.catalogGate.resolve(Response.json([createdWorkspace]));
  expect(screen.queryByText("New Workspace")).toBeNull();
  expect(screen.queryByLabelText("Workspace display name")).toBeNull();
});
test("dashboard does not contact catalog for anonymous or unknown session", async () => {
  const { state, workspaceCalls } = boundary();
  const view = render(Dashboard);
  await screen.findByRole("link", { name: "Log in" });
  expect(workspaceCalls()).toEqual([]);
  view.unmount();
  state.whoamiStatus = 503;
  render(Dashboard);
  await screen.findByRole("alert");
  expect(workspaceCalls()).toEqual([]);
});
