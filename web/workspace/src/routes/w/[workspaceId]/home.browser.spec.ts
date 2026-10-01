// @vitest-environment happy-dom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import type { ComponentProps } from "svelte";
import { invalidate } from "$app/navigation";
import Home from "./+page.svelte";
import { load } from "./+page";
import {
  type Dashboard,
  type DashboardFeed,
  loadDashboard,
  recentRows,
} from "$lib/workspace/home/dashboard";
import {
  dashboardFixture,
  fixtureDetail,
} from "$lib/workspace/home/dashboard.test-fixtures";

vi.mock("$app/navigation", () => ({ invalidate: vi.fn(async () => {}) }));
type Data = ComponentProps<typeof Home>["data"];
function fixtureFetch(failPath?: string) {
  return vi.fn(async (path: RequestInfo | URL, _init?: RequestInit) => {
    const url = new URL(String(path), "https://example.test");
    const [, id, resource] = url.pathname.match(/^\/api\/w\/([^/]+)(\/.*)$/)!;
    return resource === failPath
      ? Response.json({}, { status: 503 })
      : Response.json(
        dashboardFixture(resource, url.searchParams, decodeURIComponent(id)),
      );
  });
}
function data(id = "home-owner", fetchFn: typeof fetch = fixtureFetch()): Data {
  return {
    workspaceId: id,
    workspace: { workspace_id: id, display_name: "Home review" },
    dashboard: loadDashboard(fetchFn, id),
  } as unknown as Data;
}
function mount(value = data()) {
  return render(Home, {
    props: { params: { workspaceId: value.workspaceId }, data: value },
  });
}
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

test("Home shows actual reviews, active assignments/blockers, and recent records, not navigation tiles", async () => {
  mount();
  expect(await screen.findByText("Changes requested")).toBeTruthy();
  expect(await screen.findByText("Blocked by T-99")).toBeTruthy();
  expect(screen.getByText("W-7")).toBeTruthy();
  expect(await screen.findByText("Improve daily Workspace operation"))
    .toBeTruthy();
  expect(screen.getByText("Review pending")).toBeTruthy();
  expect(screen.getByText("Approved")).toBeTruthy();
  expect(screen.queryByRole("navigation")).toBeNull();
  expect(screen.queryByRole("heading", { name: "Settings" })).toBeNull();
  expect(screen.getAllByRole("button")).toHaveLength(1);
  expect(document.body.textContent).not.toMatch(
    /internal-worker|internal-merge|internal-runtime|Record authority/,
  );
  for (const link of screen.getAllByRole("link")) {
    expect(link.getAttribute("href")).toMatch(
      /^\/w\/home-owner\/(tickets|objectives|merge-requests)\/[^/]+$/,
    );
  }
});

test("loader only uses bounded read-only supported endpoints and advertises explicit refresh dependency", async () => {
  const fetch = fixtureFetch();
  const depends = vi.fn();
  const result = await load(
    {
      fetch,
      params: { workspaceId: "home-owner" },
      depends,
    } as unknown as Parameters<typeof load>[0],
  );
  await Promise.all(Object.values(result!.dashboard));
  expect(depends).toHaveBeenCalledWith("workspace:home");
  expect(fetch).toHaveBeenCalledTimes(7);
  for (const [path, init] of fetch.mock.calls) {
    const url = new URL(String(path), "https://example.test");
    expect(init?.method ?? "GET").toBe("GET");
    expect(init?.signal).toBeTruthy();
    expect(url.pathname).not.toMatch(/query|hosts|workers|settings/);
    if (!url.pathname.includes("/tickets/T-")) {
      expect(url.searchParams.get("limit")).toBe("5");
    }
  }
  expect(fetch.mock.calls.some(([url]) => String(url).includes("states=done")))
    .toBe(true);
  expect(
    fetch.mock.calls.some(([url]) => String(url).includes("states=closed")),
  ).toBe(true);
});

test("recent updates merge independently limited sources in actual update order, not state priority", async () => {
  const feed = await data().dashboard.recent;
  expect(feed.errors).toEqual([]);
  expect(feed.rows.map((row) => row.reference)).toEqual([
    "T-105",
    "O-12",
    "T-103",
    "T-104",
  ]);
  const rows = Array.from(
    { length: 10 },
    (_, index) => ({
      ...feed.rows[0],
      key: String(index),
      updatedAt: index === 9 ? null : `2026-01-01T0${index}:00:00Z`,
    }),
  );
  expect(
    recentRows([...rows, { ...rows[8], updatedAt: rows[0].updatedAt }]).map((
      row,
    ) => row.key),
  ).toEqual([
    "8",
    "7",
    "6",
    "5",
    "4",
  ]);
});

test("partial failure is not an empty/all-clear state and leaves sibling sections usable", async () => {
  mount(data("home-owner", fixtureFetch("/merge-requests")));
  expect((await screen.findByRole("alert")).textContent).toContain(
    "Merge Requests",
  );
  expect(screen.queryByText("No open Merge Requests.")).toBeNull();
  expect(await screen.findByText("Blocked by T-99")).toBeTruthy();
  expect(await screen.findByText("Improve daily Workspace operation"))
    .toBeTruthy();
});

test("one recent source failure preserves the other records and does not assert an empty feed", async () => {
  const value = data("home-owner", fixtureFetch("/objectives"));
  mount(value);
  expect((await value.dashboard.recent).rows.map((row) => row.reference))
    .toEqual(["T-105", "T-103", "T-104"]);
  expect((await screen.findByRole("alert")).textContent).toContain(
    "Objectives",
  );
  expect(screen.queryByText("No done / closed tickets or Objectives yet."))
    .toBeNull();
});

test("a failed Ticket detail preserves the summary without inventing no-assignee or unblocked state", async () => {
  const feed = await data("home-owner", fixtureFetch("/tickets/T-101"))
    .dashboard.active;
  expect(feed.rows[0].detail).toBe("Assignment and blockers unavailable");
  expect(feed.rows[0].workerKey).toBeUndefined();
  expect(feed.rows[0].attention).toBeUndefined();
  expect(feed.errors[0]).toContain("T-101");
});

test("a concurrent Ticket transition leaves the active scope with a refresh notice", async () => {
  const fallback = fixtureFetch();
  const fetchFn: typeof fetch = (path, init) =>
    String(path).endsWith("/tickets/T-101")
      ? Promise.resolve(Response.json({ ...fixtureDetail(), state: "done" }))
      : fallback(path, init);
  const feed = await data("home-owner", fetchFn).dashboard.active;
  expect(feed.rows.map((row) => row.reference)).toEqual(["T-102"]);
  expect(feed.notice).toContain("Work changed");
});

test("empty resources have scoped empty states and no fabricated totals", async () => {
  mount(data("home-empty"));
  expect(await screen.findByText("No open Merge Requests.")).toBeTruthy();
  expect(await screen.findByText("No in-progress or queued tickets."))
    .toBeTruthy();
  expect(await screen.findByText("No done / closed tickets or Objectives yet."))
    .toBeTruthy();
  expect(screen.queryAllByRole("link")).toHaveLength(0);
});

test("slow review loading does not hide the other completed sections", async () => {
  const value = data();
  value.dashboard.reviews = new Promise(() => {});
  mount(value);
  expect(
    within(screen.getByRole("region", { name: "Needs attention" })).getByRole(
      "status",
    ).textContent,
  ).toContain("Loading needs attention");
  expect(await screen.findByText("Blocked by T-99")).toBeTruthy();
  expect(await screen.findByText("Improve daily Workspace operation"))
    .toBeTruthy();
});

test("route reuse drops pending old Workspace results and updates scoped links", async () => {
  let finish!: (feed: DashboardFeed) => void;
  const value = data();
  value.dashboard.reviews = new Promise((resolve) => {
    finish = resolve;
  });
  const view = mount(value);
  await view.rerender({ data: data("space / 日本") });
  await screen.findByText("Changes requested");
  finish({ rows: [], errors: ["OLD WORKSPACE ERROR"] });
  await Promise.resolve();
  expect(screen.queryByText("OLD WORKSPACE ERROR")).toBeNull();
  for (const link of screen.getAllByRole("link")) {
    expect(link.getAttribute("href")).toContain(
      "/w/space%20%2F%20%E6%97%A5%E6%9C%AC/",
    );
  }
});

test("malformed review responses are unavailable rather than empty", async () => {
  const fallback = fixtureFetch();
  const fetchFn: typeof fetch = (path, init) =>
    String(path).includes("/merge-requests?")
      ? Promise.resolve(Response.json({ items: "invalid" }))
      : fallback(path, init);
  const feed = await data("home-owner", fetchFn).dashboard.reviews;
  expect(feed.rows).toEqual([]);
  expect(feed.errors).toEqual([
    "Merge Requests unavailable. Refresh to retry.",
  ]);
});

test("unknown review states are preserved and branch text never becomes HTML", async () => {
  const fallback = fixtureFetch();
  const fetchFn: typeof fetch = async (path, init) => {
    const response = await fallback(path, init);
    if (!String(path).includes("/merge-requests?")) return response;
    const body = await response.json();
    body.items[0].summary.review_status = "future_status";
    body.items[0].summary.selector_from = '<img src=x onerror="alert(1)">';
    return Response.json(body);
  };
  const view = mount(data("home-owner", fetchFn));
  expect(await screen.findByText("future_status")).toBeTruthy();
  expect(view.container.querySelector("img")).toBeNull();
  expect(await screen.findByText(/<img src=x/)).toBeTruthy();
});

test("unavailable timestamps are not presented as current activity", async () => {
  const value = data();
  const feed = await value.dashboard.recent;
  feed.rows[0].updatedAt = "invalid";
  value.dashboard.recent = Promise.resolve(feed);
  mount(value);
  expect(await screen.findByText("Update time unavailable")).toBeTruthy();
});

test("Refresh invalidates only the Home data dependency", async () => {
  const value = data();
  mount(value);
  await Promise.all(Object.values(value.dashboard));
  await fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  expect(invalidate).toHaveBeenCalledWith("workspace:home");
});
