// @vitest-environment happy-dom
import { cleanup, render, screen } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import SubjectsNavSection from "../sidebar/SubjectsNavSection.svelte";
import WorkspaceSidebar from "../sidebar/WorkspaceSidebar.svelte";

vi.mock("#lib/workspace/sidebar/worker-subscription", () => ({
  workspaceWorkersStore: () => ({
    subscribe: (
      subscriber: (state: { loading: boolean; workers: never[] }) => void,
    ) => {
      subscriber({ loading: false, workers: [] });
      return () => {};
    },
  }),
  refreshWorkspaceWorkers: async () => {},
}));

afterEach(cleanup);
test("Subjects is a direct resource link and follows route changes", async () => {
  const view = render(SubjectsNavSection, {
    workspaceId: "w/s",
    currentPath: "/w/w%2Fs/subjektiv/a/chat",
  });
  const subjects = screen.getByRole("link", { name: "Subjects" });
  expect(subjects.getAttribute("href")).toBe("/w/w%2Fs/subjektiv");
  expect(subjects.getAttribute("aria-current")).toBe("page");
  expect(subjects.closest("section")?.querySelector("h2")).toBeNull();
  await view.rerender({
    workspaceId: "w/s",
    currentPath: "/w/w%2Fs/subjektiv-other",
  });
  expect(subjects.getAttribute("aria-current")).toBeNull();
});
test("Workspace resource navigation exposes Subjects without retired Workspace Memory", () => {
  render(WorkspaceSidebar, {
    workspace: {
      workspace_id: "w",
      display_name: "Workspace",
      record_authority: "fixture",
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
        manage_repositories: false,
        manage_secrets: false,
        manage_runtimes: false,
        delete_workspace: false,
        delete_subjects: false,
      },
      extension_points: {
        store: "fixture",
        event_stream: {
          status: "unavailable",
          note: "fixture",
          diagnostics: [],
        },
        host_worker_bridge: {
          status: "unavailable",
          note: "fixture",
          diagnostics: [],
        },
        companion_console: {
          status: "unavailable",
          note: "fixture",
          diagnostics: [],
        },
      },
    },
    currentPath: "/w/w/subjektiv",
  });
  expect(
    screen.getByRole("link", { name: "Subjects" }).getAttribute("aria-current"),
  ).toBe("page");
  expect(screen.queryByRole("link", { name: "Staging" })).toBeNull();
  expect(screen.queryByRole("heading", { name: "Memory" })).toBeNull();
  expect(document.querySelector('a[href*="/memory"]')).toBeNull();
});
test("Retired Workspace staging URL never activates the Subject resource", () => {
  render(SubjectsNavSection, {
    workspaceId: "w",
    currentPath: "/w/w/memory/staging",
  });
  expect(
    screen.getByRole("link", { name: "Subjects" }).getAttribute("aria-current"),
  ).toBeNull();
});
