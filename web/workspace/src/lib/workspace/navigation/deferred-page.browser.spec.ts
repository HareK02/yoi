// @vitest-environment happy-dom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import { createRawSnippet, tick } from "svelte";
import DeferredPage from "./DeferredPage.svelte";
import DeferredPageHarness from "./DeferredPageHarness.svelte";
import { type DeferredResult, settle } from "./deferred.ts";

const navigation = vi.hoisted(() => ({
  goto: vi.fn(async () => {}),
  invalidateAll: vi.fn(async () => {}),
  to: null as { url: URL } | null,
}));
vi.mock("$app/navigation", () => navigation);
vi.mock(
  "$app/state",
  () => ({ page: { url: new URL("https://example.test/w/team/tickets") }, navigating: navigation }),
);
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  navigation.to = null;
  navigation.invalidateAll.mockImplementation(async () => {});
});
function gate<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve: (value: T) => resolve(value) };
}
type Content = { label: string };
const TestPage = DeferredPage<
  { deferredPage: Promise<DeferredResult<Content>> }
>;
const children = createRawSnippet<[Content]>((getData) => ({
  render: () =>
    `<div><label>Draft<input aria-label="Draft" /></label><p>${getData().label}</p></div>`,
}));

test("pending route shows destination loading without constructing its editor", async () => {
  const pending = gate<DeferredResult<Content>>();
  const view = render(TestPage, {
    data: { deferredPage: pending.promise },
    title: "Tickets",
    children,
  });
  expect(view.getByRole("heading").textContent).toBe("Tickets");
  expect(view.getByRole("status").textContent).toBe("Loading…");
  expect(view.queryByLabelText("Draft")).toBeNull();
  pending.resolve({ ok: true, data: { label: "First page" } });
  await view.findByLabelText("Draft");
  expect(view.queryByRole("status")).toBeNull();
});

test("same-path refresh keeps the mounted input and pending edits", async () => {
  const view = render(TestPage, {
    data: { deferredPage: settle({ label: "First page" }) },
    title: "Tickets",
    children,
  });
  const input = await view.findByLabelText("Draft") as HTMLInputElement;
  await fireEvent.input(input, { target: { value: "Unsent draft" } });
  const refresh = gate<DeferredResult<Content>>();
  await view.rerender({
    data: { deferredPage: refresh.promise },
    title: "Tickets",
    children,
  });
  expect(view.getByRole("status").textContent).toBe("Updating…");
  expect(view.getByLabelText("Draft")).toBe(input);
  refresh.resolve({ ok: true, data: { label: "Fresh data" } });
  await waitFor(() => expect(view.queryByRole("status")).toBeNull());
  expect(view.getByLabelText("Draft")).toBe(input);
  expect(input.value).toBe("Unsent draft");
});

test("late previous load cannot redirect or overwrite a newer result", async () => {
  const old = gate<DeferredResult<Content>>();
  const view = render(TestPage, {
    data: { deferredPage: old.promise },
    title: "Tickets",
    children,
  });
  await view.rerender({
    data: { deferredPage: settle({ label: "Current page" }) },
    title: "Tickets",
    children,
  });
  await view.findByText("Current page");
  old.resolve({
    ok: false,
    failure: { message: "Redirecting…", status: 308, redirect: "/stale" },
  });
  await tick();
  expect(navigation.goto).not.toHaveBeenCalled();
  expect(view.getByText("Current page")).toBeTruthy();
});

test("permission failure never renders content and Retry requests fresh load", async () => {
  const view = render(TestPage, {
    data: {
      deferredPage: Promise.resolve({
        ok: false as const,
        failure: { message: "Permission denied", status: 403, redirect: null },
      }),
    },
    title: "Settings",
    children,
  });
  await view.findByRole("alert");
  expect(view.queryByLabelText("Draft")).toBeNull();
  await fireEvent.click(view.getByRole("button", { name: "Retry" }));
  expect(navigation.invalidateAll).toHaveBeenCalledOnce();
});

test("denied refresh clears cached controls throughout a pending Retry", async () => {
  const view = render(TestPage, {data:{deferredPage:settle({label:"Authorized"})}, title:"Settings", children});
  await view.findByLabelText("Draft");
  await view.rerender({data:{deferredPage:Promise.resolve({ok:false, failure:{message:"Denied",status:403,redirect:null}})}, title:"Settings", children});
  await view.findByRole("alert");
  const retry=gate<DeferredResult<Content>>();
  await view.rerender({data:{deferredPage:retry.promise},title:"Settings",children});
  expect(view.queryByLabelText("Draft")).toBeNull();
  expect(view.getByRole("status").textContent).toBe("Loading…");
});

test("query-derived launch identity remounts the editor even on the same pathname", async () => {
  const view=render(TestPage,{data:{deferredPage:settle({label:"Ticket A"})},title:"New Worker",identity:"team:ticket-a",children});
  const old=await view.findByLabelText("Draft") as HTMLInputElement;
  old.value="Ticket A instruction";
  await view.rerender({data:{deferredPage:settle({label:"Ticket B"})},title:"New Worker",identity:"team:ticket-b",children});
  await view.findByText("Ticket B");
  expect(view.getByLabelText("Draft")).not.toBe(old);
  expect((view.getByLabelText("Draft") as HTMLInputElement).value).toBe("");
});

test("old canonical redirect cannot overtake a navigation still loading its route module", async () => {
  const old=gate<DeferredResult<Content>>();
  render(TestPage,{data:{deferredPage:old.promise},title:"Tickets",children});
  navigation.to={url:new URL("https://example.test/w/team/objectives")};
  old.resolve({ok:false,failure:{message:"Redirecting…",status:308,redirect:"/stale"}});
  await tick();
  expect(navigation.goto).not.toHaveBeenCalled();
});

function expectWorkspacePreserved() {
  expect(screen.getByText("Valid Workspace")).toBeTruthy();
  expect(screen.getByRole("complementary", { name: "Workspace sidebar" })).toBeTruthy();
  expect(screen.queryByText("Workspace unavailable")).toBeNull();
  expect(screen.queryByText("Workspace removed")).toBeNull();
  expect(screen.queryByRole("button", { name: "Create Workspace" })).toBeNull();
}

const renderError = 'Ticket lookup failed: <img src=x onerror="throw 1">';
const brokenData = () => ({ deferredPage: settle({ label: "Broken ticket", message: renderError }) });

test("child constructor error stays local, with the actual message escaped below the page title", async () => {
  const view = render(DeferredPageHarness, { data: brokenData(), identity: "team:ticket-a" });
  const alert = await view.findByRole("alert");
  expect(view.getByRole("heading", { name: "Ticket" })).toBeTruthy();
  expect(alert.textContent).toBe(renderError);
  expect(alert.querySelector("img")).toBeNull();
  expect(view.container.querySelector("main h1 + p")).toBe(alert);
  expect(view.queryByLabelText("Draft")).toBeNull();
  expectWorkspacePreserved();
});

test("serialized SvelteKit App.Error displays its message, not object coercion", async () => {
  const view = render(DeferredPageHarness, {
    data: { deferredPage: settle({ label: "Broken ticket", message: renderError, serializedError: true }) },
    identity: "team:ticket-a",
  });
  const alert = await view.findByRole("alert");
  expect(alert.textContent).toBe(renderError);
  expect(alert.querySelector("img")).toBeNull();
  expectWorkspacePreserved();
});

test("Retry waits for new successful deferred data before remounting a failed child", async () => {
  const onConstruct = vi.fn();
  const view = render(DeferredPageHarness, { data: brokenData(), identity: "team:ticket-a", onConstruct });
  await view.findByRole("alert");
  const invalidation = gate<void>();
  navigation.invalidateAll.mockImplementationOnce(() => invalidation.promise);
  await fireEvent.click(view.getByRole("button", { name: "Retry" }));
  expect(navigation.invalidateAll).toHaveBeenCalledOnce();
  expect(onConstruct).toHaveBeenCalledTimes(1);
  expect((view.getByRole("button", { name: "Retry" }) as HTMLButtonElement).disabled).toBe(true);

  const refresh = gate<DeferredResult<Content>>();
  await view.rerender({ data: { deferredPage: refresh.promise }, identity: "team:ticket-a", onConstruct });
  invalidation.resolve();
  await tick();
  expect(view.getByRole("alert").textContent).toBe(renderError);
  expect(onConstruct).toHaveBeenCalledTimes(1);
  expect((view.getByRole("button", { name: "Retry" }) as HTMLButtonElement).disabled).toBe(true);
  expectWorkspacePreserved();

  refresh.resolve({ ok: true, data: { label: "Recovered ticket" } });
  await view.findByText("Recovered ticket");
  expect(onConstruct).toHaveBeenCalledTimes(2);
  expect(view.queryByRole("alert")).toBeNull();
  expect(view.queryByRole("status")).toBeNull();
  expect(view.getByLabelText("Draft")).toBeTruthy();
  expectWorkspacePreserved();
});

test("invalidation without new data never resets the boundary against stale failed data", async () => {
  const onConstruct = vi.fn();
  const view = render(DeferredPageHarness, { data: brokenData(), identity: "team:ticket-a", onConstruct });
  await view.findByRole("alert");
  await fireEvent.click(view.getByRole("button", { name: "Retry" }));
  await tick();
  expect(onConstruct).toHaveBeenCalledTimes(1);
  expect(view.getByRole("alert").textContent).toBe(renderError);
  expectWorkspacePreserved();
});

test("failed replacement data remains local and another fresh result can recover", async () => {
  const onConstruct = vi.fn();
  const view = render(DeferredPageHarness, { data: brokenData(), identity: "team:ticket-a", onConstruct });
  await view.findByRole("alert");
  await view.rerender({
    data: { deferredPage: settle({ label: "Still broken", message: "Ticket still missing" }) },
    identity: "team:ticket-a", onConstruct,
  });
  await waitFor(() => expect(view.getByRole("alert").textContent).toBe("Ticket still missing"));
  expect(onConstruct).toHaveBeenCalledTimes(2);
  expectWorkspacePreserved();
  await view.rerender({ data: { deferredPage: settle({ label: "Recovered" }) }, identity: "team:ticket-a", onConstruct });
  await view.findByText("Recovered");
  expect(view.queryByRole("alert")).toBeNull();
});

test("changing route identity clears a failed boundary and rejects late cross-workspace results", async () => {
  const onConstruct = vi.fn();
  const view = render(DeferredPageHarness, { data: brokenData(), identity: "team:ticket-a", onConstruct });
  await view.findByRole("alert");
  const stale = gate<DeferredResult<Content>>();
  await view.rerender({ data: { deferredPage: stale.promise }, identity: "team:ticket-a", onConstruct });
  const workspace = gate<DeferredResult<{ workspace: string }>>();
  const destination = gate<DeferredResult<Content>>();
  await view.rerender({
    data: { deferredWorkspace: workspace.promise, deferredPage: destination.promise },
    identity: "other-team:ticket-b", onConstruct,
  });
  expect(view.queryByRole("alert")).toBeNull();
  expect(view.getByRole("status").textContent).toBe("Loading…");
  expect(view.queryByLabelText("Draft")).toBeNull();
  destination.resolve({ ok: true, data: { label: "Other Workspace ticket" } });
  stale.resolve({ ok: true, data: { label: "Stale team ticket" } });
  await tick();
  expect(onConstruct).toHaveBeenCalledTimes(1);
  expect(view.queryByText("Stale team ticket")).toBeNull();
  workspace.resolve({ ok: true, data: { workspace: "other-team" } });
  await view.findByText("Other Workspace ticket");
  expect(onConstruct.mock.calls.map(([label]) => label)).toEqual(["Broken ticket", "Other Workspace ticket"]);
  expect(view.queryByRole("alert")).toBeNull();
  expectWorkspacePreserved();
});

test("a failed deferred Retry never reconstructs the old failed child", async () => {
  const onConstruct = vi.fn();
  const view = render(DeferredPageHarness, { data: brokenData(), identity: "team:ticket-a", onConstruct });
  await view.findByRole("alert");
  await view.rerender({
    data: { deferredPage: Promise.resolve({ ok: false, failure: { message: "Ticket access denied", status: 403, redirect: null } }) },
    identity: "team:ticket-a", onConstruct,
  });
  await waitFor(() => expect(view.getByRole("alert").textContent).toBe("Ticket access denied"));
  expect(onConstruct).toHaveBeenCalledTimes(1);
  expect(view.queryByLabelText("Draft")).toBeNull();
  expectWorkspacePreserved();
  await view.rerender({ data: { deferredPage: settle({ label: "Authorized ticket" }) }, identity: "team:ticket-a", onConstruct });
  await view.findByText("Authorized ticket");
  expect(onConstruct).toHaveBeenCalledTimes(2);
  expect(view.queryByRole("alert")).toBeNull();
});

test("a failed Retry request is handled locally without losing the original render error", async () => {
  const view = render(DeferredPageHarness, { data: brokenData(), identity: "team:ticket-a" });
  await view.findByRole("alert");
  navigation.invalidateAll.mockRejectedValueOnce(new Error("Reload failed"));
  await fireEvent.click(view.getByRole("button", { name: "Retry" }));
  await view.findByText("Reload failed");
  expect(view.getByText(renderError)).toBeTruthy();
  expect((view.getByRole("button", { name: "Retry" }) as HTMLButtonElement).disabled).toBe(false);
  expectWorkspacePreserved();
});
