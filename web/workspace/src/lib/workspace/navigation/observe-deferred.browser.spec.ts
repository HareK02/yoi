// @vitest-environment happy-dom
import { cleanup, render, waitFor } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import { tick } from "svelte";
import ObserveDeferredHarness from "./ObserveDeferredHarness.svelte";
import { type DeferredResult, settle } from "./deferred.ts";

afterEach(cleanup);

function gate() {
  let resolve!: (value: DeferredResult<string>) => void;
  const promise = new Promise<DeferredResult<string>>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

test("missing route identity neither dereferences empty state nor reads removed layout data", () => {
  const read = vi.fn(() => {
    throw new Error("layout data is no longer available");
  });
  const view = render(ObserveDeferredHarness, { identity: undefined, read });
  expect(view.getByTestId("failure").textContent).toBe("no failure");
  expect(view.getByTestId("data").textContent).toBe("no data");
  expect(read).not.toHaveBeenCalled();
});

test("leaving a loaded Workspace hides its data and can enter another Workspace", async () => {
  const view = render(ObserveDeferredHarness, {
    identity: "team",
    read: () => settle("Team"),
  });
  await view.findByText("Team");
  const removed = vi.fn(() => {
    throw new Error("removed layout data");
  });
  await view.rerender({ identity: undefined, read: removed });
  expect(view.getByTestId("failure").textContent).toBe("no failure");
  expect(view.getByTestId("data").textContent).toBe("no data");
  expect(removed).not.toHaveBeenCalled();
  await view.rerender({
    identity: "other",
    read: () => settle("Other Workspace"),
  });
  await view.findByText("Other Workspace");
});

test("late results after leaving a Workspace cannot restore its data or failure", async () => {
  for (
    const result of [
      { ok: true, data: "Old Workspace" },
      {
        ok: false,
        failure: { message: "Old failure", status: 403, redirect: null },
      },
    ] satisfies DeferredResult<string>[]
  ) {
    const pending = gate();
    const view = render(ObserveDeferredHarness, {
      identity: "team",
      read: () => pending.promise,
    });
    await view.rerender({ identity: undefined, read: () => pending.promise });
    pending.resolve(result);
    await tick();
    expect(view.getByTestId("failure").textContent).toBe("no failure");
    expect(view.getByTestId("data").textContent).toBe("no data");
    view.unmount();
  }
});

test("same-Workspace refresh retains data, while denied refresh and identity changes hide it", async () => {
  const view = render(ObserveDeferredHarness, {
    identity: "team",
    read: () => settle("Team"),
  });
  await view.findByText("Team");
  const refresh = gate();
  await view.rerender({ identity: "team", read: () => refresh.promise });
  expect(view.getByTestId("data").textContent).toBe("Team");
  refresh.resolve({
    ok: false,
    failure: { message: "Denied", status: 403, redirect: null },
  });
  await view.findByText("Denied");
  expect(view.getByTestId("data").textContent).toBe("no data");
  await view.rerender({ identity: undefined, read: () => refresh.promise });
  await waitFor(() =>
    expect(view.getByTestId("failure").textContent).toBe("no failure")
  );
});
