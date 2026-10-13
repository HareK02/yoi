// @vitest-environment happy-dom
import { cleanup, fireEvent, render, waitFor } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import { createRawSnippet, tick } from "svelte";
import DeferredPage from "./DeferredPage.svelte";
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
