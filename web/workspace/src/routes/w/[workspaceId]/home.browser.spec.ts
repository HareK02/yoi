// @vitest-environment happy-dom
import { cleanup, render, screen, within } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import type { ComponentProps } from "svelte";
import Home from "./+page.svelte";
import { load } from "./+page";
import { parseWorkspaceResponse } from "$lib/workspace/api/workspace-model";

type Data = ComponentProps<typeof Home>["data"];
function homeData(owner = true, id = "home-review"): Data {
  return {
    workspaceId: id,
    workspace: parseWorkspaceResponse({
      workspace_id: id,
      display_name: "Workspace Home Review",
      record_authority: "private-authority",
      schema_version: 1,
      auth: {
        Passkey: {
          rp_id: "example.test",
          origin: "https://example.test",
          public_base_url: "https://example.test",
          cookie_name: "fixture",
        },
      },
      permissions: {
        manage_repositories: owner,
        manage_secrets: owner,
        manage_runtimes: owner,
        delete_workspace: owner,
      },
      extension_points: {
        store: "private-store",
        event_stream: { status: "ready", note: "", diagnostics: [] },
        host_worker_bridge: { status: "ready", note: "", diagnostics: [] },
        companion_console: { status: "ready", note: "", diagnostics: [] },
      },
    }),
    workspaceError: null,
    repositories: null,
    repositoriesError: null,
    accessibleWorkspaces: [],
    workspaceCatalogError: null,
  } as Data;
}
afterEach(cleanup);

test("Home presents daily resources in product order, without internal metadata or repeated identity", () => {
  const { container } = render(Home, {
    props: { params: { workspaceId: "home-review" }, data: homeData() },
  });
  const work = screen.getByRole("navigation", { name: "Work" });
  expect(
    within(work).getAllByRole("link").map((link) => link.textContent?.trim()),
  ).toEqual([
    "Tickets",
    "Objectives",
    "Merge Requests",
    "Memory",
    "Workers",
  ]);
  expect(
    within(work).getAllByRole("link").map((link) => link.getAttribute("href")),
  ).toEqual([
    "/w/home-review/tickets",
    "/w/home-review/objectives",
    "/w/home-review/merge-requests",
    "/w/home-review/memory",
    "/w/home-review/workers",
  ]);
  expect(container.querySelectorAll("h1")).toHaveLength(1);
  expect(container.textContent).not.toMatch(
    /private-authority|private-store|home-review|Workspace Home Review|Hosts|Record authority|API/,
  );
  expect(
    container.querySelectorAll(
      "button, .card, .workspace-action-card, .state-pill",
    ),
  ).toHaveLength(0);
});

test("owner management destinations are separate from daily resources", () => {
  render(Home, {
    props: { params: { workspaceId: "home-review" }, data: homeData() },
  });
  const settings = screen.getByRole("navigation", { name: "Settings" });
  expect(within(settings).getAllByRole("link").map((link) => link.textContent))
    .toEqual([
      "Runtimes",
      "Configuration Sources",
      "Repositories",
      "Repository Access",
      "Profile Sources",
      "Workspace Identity",
    ]);
});

test("non-owner retains read destinations without owner-only controls", () => {
  render(Home, {
    props: { params: { workspaceId: "home-review" }, data: homeData(false) },
  });
  const settings = screen.getByRole("navigation", { name: "Settings" });
  expect(within(settings).getAllByRole("link").map((link) => link.textContent))
    .toEqual(["Configuration Sources", "Profile Sources"]);
  expect(screen.getAllByRole("link")).toHaveLength(7);
});

test.each(
  [
    ["manage_runtimes", "Runtimes"],
    ["manage_repositories", "Repositories"],
    ["manage_secrets", "Repository Access"],
    ["delete_workspace", "Workspace Identity"],
  ] as const,
)(
  "%s alone exposes only its matching management destination",
  (permission, label) => {
    const data = homeData(false);
    data.workspace!.permissions[permission] = true;
    render(Home, { props: { params: { workspaceId: "home-review" }, data } });
    const settings = screen.getByRole("navigation", { name: "Settings" });
    expect(within(settings).getAllByRole("link")).toHaveLength(3);
    expect(within(settings).getByRole("link", { name: label })).toBeTruthy();
  },
);

test("route reuse updates all destinations and permissions from the new Workspace", async () => {
  const view = render(Home, {
    props: { params: { workspaceId: "home-review" }, data: homeData() },
  });
  await view.rerender({ data: homeData(false, "space / 日本") });
  expect(screen.queryByRole("link", { name: "Runtimes" })).toBeNull();
  for (const link of screen.getAllByRole("link")) {
    expect(link.getAttribute("href")).toMatch(
      /^\/w\/space%20%2F%20%E6%97%A5%E6%9C%AC\//,
    );
  }
});

test("loading and failure do not invent resources or management permissions", async () => {
  const data = { ...homeData(), workspace: null } as unknown as Data;
  const view = render(Home, {
    props: { params: { workspaceId: "home-review" }, data },
  });
  expect(screen.getByRole("status").textContent).toBe("Loading workspace…");
  expect(screen.queryAllByRole("link")).toHaveLength(0);
  await view.rerender({
    data: {
      ...data,
      workspaceError: "Workspace access unavailable",
    } as unknown as Data,
  });
  expect(screen.getByRole("alert").textContent).toBe(
    "Workspace access unavailable",
  );
  expect(
    screen.getByRole("link", { name: "Choose a workspace" }).getAttribute(
      "href",
    ),
  ).toBe("/");
  expect(screen.queryByRole("navigation")).toBeNull();
});

test("Home does not fetch infrastructure inventories", async () => {
  const fetch = vi.fn();
  expect(
    await load(
      {
        params: { workspaceId: "home-review" },
        fetch,
      } as unknown as Parameters<typeof load>[0],
    ),
  ).toEqual({ workspaceId: "home-review" });
  expect(fetch).not.toHaveBeenCalled();
});
