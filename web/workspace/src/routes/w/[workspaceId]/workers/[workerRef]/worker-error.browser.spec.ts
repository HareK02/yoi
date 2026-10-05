// @vitest-environment happy-dom

import { cleanup, fireEvent, render } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import WorkerError from "./+error.svelte";

vi.mock("$app/state", () => ({
  page: { params: { workspaceId: "workspace-a", workerRef: "worker-a" }, status: 500 },
}));

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

test("Worker route failures offer local recovery instead of claiming Workspace removal", async () => {
  const reload = vi.spyOn(window.location, "reload").mockImplementation(() => {});
  const view = render(WorkerError);
  expect(view.getByRole("heading", { name: "Unable to open this Worker page" })).toBeTruthy();
  expect(view.getByRole("link", { name: "Back to Workers" }).getAttribute("href"))
    .toBe("/w/workspace-a/workers");
  expect(view.queryByText("The selected Workspace cannot be opened")).toBeNull();
  expect(view.queryByText("Create Workspace")).toBeNull();
  await fireEvent.click(view.getByRole("button", { name: "Reload page" }));
  expect(reload).toHaveBeenCalledOnce();
});
