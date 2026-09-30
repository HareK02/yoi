// @vitest-environment happy-dom

import { cleanup, fireEvent, render } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import ConsoleTasks from "./ConsoleTasks.svelte";
import type { ConsoleTask } from "./tasks";

const tasks: ConsoleTask[] = [{ taskid: 1, subject: "Current task", description: "Task detail", status: "inprogress" }];
afterEach(cleanup);

test("mini summary opens the pane independently of Worker view tabs", async () => {
  const onTogglePane = vi.fn();
  const onSelectWorkerView = vi.fn();
  const view = render(ConsoleTasks, {
    tasks, mode: "mini", paneId: "task-pane", paneOpen: false, onTogglePane,
    workerViews: [{ sessionId: null, label: "Main" }, { sessionId: "child", label: "Child" }],
    onSelectWorkerView,
  });
  const summary = view.getByRole("button", { name: /1 task — pending/ });
  expect(summary.getAttribute("aria-expanded")).toBe("false");
  expect(summary.getAttribute("aria-controls")).toBe("task-pane");
  expect(summary.getAttribute("type")).toBe("button");
  await fireEvent.click(view.getByText("Current task"));
  expect(onTogglePane).not.toHaveBeenCalled();
  await fireEvent.click(summary);
  expect(onTogglePane).toHaveBeenCalledOnce();
  await view.rerender({ paneOpen: true });
  expect(summary.getAttribute("aria-expanded")).toBe("true");
  await fireEvent.click(view.getByRole("button", { name: "Child" }));
  expect(onSelectWorkerView).toHaveBeenCalledWith("child");
  expect(onTogglePane).toHaveBeenCalledOnce();
  expect(summary.querySelector("button")).toBeNull();
});

test("an open empty pane retains its mini summary close control", async () => {
  const onTogglePane = vi.fn();
  const view = render(ConsoleTasks, { tasks: [], mode: "mini", paneOpen: true, onTogglePane });
  await fireEvent.click(view.getByRole("button", { name: /0 tasks/ }));
  expect(onTogglePane).toHaveBeenCalledOnce();
  await view.rerender({ paneOpen: false });
  expect(view.queryByRole("button")).toBeNull();
});

test("pane exposes the summary control target and full task details", () => {
  const view = render(ConsoleTasks, { tasks, mode: "pane", paneId: "task-pane" });
  expect(view.getByRole("complementary", { name: "Worker tasks" }).id).toBe("task-pane");
  expect(view.getByText("Task detail")).not.toBeNull();
});
