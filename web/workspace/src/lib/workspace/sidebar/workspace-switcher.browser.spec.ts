// @vitest-environment happy-dom

import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import WorkspaceAlerts from "#lib/workspace/alerts/WorkspaceAlerts.svelte";
import { clearWorkspaceAlerts } from "#lib/workspace/alerts/store.ts";
import WorkspaceSwitcher from "./WorkspaceSwitcher.svelte";

afterEach(() => {
  cleanup();
  clearWorkspaceAlerts();
  vi.unstubAllGlobals();
});

test("Workspace list fetch failure uses WorkspaceAlerts and keeps the menu free of diagnostics", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () =>
      new Response("Catalog temporarily unavailable", { status: 503 })
    ),
  );
  render(WorkspaceAlerts);
  render(WorkspaceSwitcher, {
    currentWorkspaceId: "workspace-a",
    currentWorkspaceName: "Workspace A",
    variant: "header",
  });

  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toContain("Workspace list unavailable");
  expect(alert.textContent).toContain("503");
  await fireEvent.click(screen.getByRole("button", { name: "Workspace A" }));
  const menu = screen.getByRole("menu", { name: "Workspace menu" });
  expect(menu.textContent).not.toContain("503");
  expect(menu.textContent).not.toContain("Workspace list unavailable");
  expect(screen.getByRole("menuitem", { name: "Workspace A" })).toBeTruthy();
});

test("unmounted Workspace menu ignores a delayed catalog failure", async () => {
  let reject!: (cause: Error) => void;
  vi.stubGlobal(
    "fetch",
    vi.fn(() =>
      new Promise<Response>((_resolve, rejectPromise) => {
        reject = rejectPromise;
      })
    ),
  );
  render(WorkspaceAlerts);
  const view = render(WorkspaceSwitcher, {
    currentWorkspaceId: "workspace-a",
    currentWorkspaceName: "Workspace A",
  });
  await waitFor(() => expect(fetch).toHaveBeenCalledOnce());
  view.unmount();
  reject(new Error("OLD WORKSPACE CATALOG FAILURE"));
  await Promise.resolve();
  await Promise.resolve();

  expect(document.body.textContent).not.toContain(
    "OLD WORKSPACE CATALOG FAILURE",
  );
  expect(document.querySelectorAll(".workspace-alert")).toHaveLength(0);
});
