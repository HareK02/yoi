// @vitest-environment happy-dom

import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { createRawSnippet, tick } from "svelte";
import { logout } from "#lib/workspace/auth/api.ts";
import Layout from "./+layout.svelte";

vi.mock("$app/state", () => ({ page: { url: new URL("https://example.test/") } }));
const storageKey = "yoi.sidebar.mode.v1";
const legacyKey = "yoi.sidebar.folded.v1";
const props = {
  params: {},
  children: createRawSnippet(() => ({ render: () => "<p>Page content</p>" })),
  data: { accessibleWorkspaces: [], workspaceCatalogError: null },
};
const frame = () => screen.getByRole("complementary", { name: "Sidebar" });
const folded = () => frame().classList.contains("folded");
const hoverRegion = () => frame().querySelector<HTMLElement>(".sidebar-hover-region")!;
const enter = () => fireEvent.pointerEnter(hoverRegion(), { pointerType: "mouse" });
const leave = () => fireEvent.pointerLeave(hoverRegion(), { pointerType: "mouse" });
const delay = () => vi.advanceTimersByTimeAsync(200);

beforeEach(() => { localStorage.clear(); vi.useFakeTimers(); });
afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  localStorage.clear();
});

test("Workspace catalog failure uses the shared alert without rendering diagnostics in the sidebar", async () => {
  render(Layout, {
    props: {
      ...props,
      data: {
        accessibleWorkspaces: [],
        workspaceCatalogError: "Workspace catalog request failed",
      },
    },
  });
  expect((await screen.findByRole("alert")).textContent).toContain(
    "Workspace catalog request failed",
  );
  expect(frame().textContent).not.toContain("Workspace catalog request failed");
  expect(frame().textContent).not.toContain("Workspace list unavailable");
});

test("unpinning changes mode, and pointer departure closes the preview", async () => {
  render(Layout, { props });
  expect(screen.getByRole("button", { name: "Unpin sidebar" }).getAttribute("aria-pressed")).toBe("true");
  await enter();
  screen.getByRole("button", { name: "Unpin sidebar" }).focus();
  await fireEvent.click(screen.getByRole("button", { name: "Unpin sidebar" }), { detail: 1 });
  expect(localStorage.getItem(storageKey)).toBe("hover");
  expect(folded()).toBe(false);
  await leave();
  await delay();
  expect(folded()).toBe(true);
});

test("footer unpin closes in the same update as the mode change, without the hover delay", async () => {
  render(Layout, { props });
  const button = screen.getByRole("button", { name: "Unpin sidebar" });
  button.focus();
  await fireEvent.click(button, { detail: 1 });
  expect(localStorage.getItem(storageKey)).toBe("hover");
  expect(folded()).toBe(true);
  await delay();
  expect(folded()).toBe(true);
  await fireEvent.click(button, { detail: 1 });
  expect(localStorage.getItem(storageKey)).toBe("pinned");
  expect(folded()).toBe(false);
});

test("footer pointer clicks release previous sidebar link focus before unpinning", async () => {
  render(Layout, { props });
  frame().querySelector<HTMLAnchorElement>("a")!.focus();
  await tick();
  await fireEvent.click(screen.getByRole("button", { name: "Unpin sidebar" }), { detail: 1 });
  expect(folded()).toBe(true);
});

test("keyboard mode switching preserves the focused preview", async () => {
  render(Layout, { props });
  const button = screen.getByRole("button", { name: "Unpin sidebar" });
  button.focus();
  await fireEvent.click(button, { detail: 0 });
  expect(localStorage.getItem(storageKey)).toBe("hover");
  await delay();
  expect(folded()).toBe(false);
});

test.each(["pinned", "hover"])("restores %s mode without persisting a transient open state", (mode) => {
  localStorage.setItem(storageKey, mode);
  render(Layout, { props });
  expect(folded()).toBe(mode === "hover");
  expect(localStorage.getItem(storageKey)).toBe(mode);
});

test.each([true, false])("migrates legacy folded=%s to a mode", (value) => {
  localStorage.setItem(legacyKey, String(value));
  render(Layout, { props });
  expect(localStorage.getItem(storageKey)).toBe(value ? "hover" : "pinned");
  expect(localStorage.getItem(legacyKey)).toBeNull();
  expect(folded()).toBe(value);
});

test("a saved mode takes precedence over the legacy preference", () => {
  localStorage.setItem(legacyKey, "true");
  localStorage.setItem(storageKey, "pinned");
  render(Layout, { props });
  expect(folded()).toBe(false);
});

test("hover keeps the same content node, delays closing, and cancels closing on reentry", async () => {
  localStorage.setItem(storageKey, "hover");
  render(Layout, { props });
  const content = frame().querySelector<HTMLElement>(".sidebar-frame-content")!;
  expect(content.inert).toBe(true);
  await enter();
  expect(folded()).toBe(false);
  expect(content.inert).toBe(false);
  await leave();
  await vi.advanceTimersByTimeAsync(100);
  expect(folded()).toBe(false);
  await enter();
  await delay();
  expect(folded()).toBe(false);
  await leave();
  await delay();
  expect(folded()).toBe(true);
  expect(frame().querySelector(".sidebar-frame-content")).toBe(content);
  expect(content.inert).toBe(true);
  expect(content.getAttribute("aria-hidden")).toBe("true");
  expect(localStorage.getItem(storageKey)).toBe("hover");
});

test("footer hover does not open or hold the preview, but its button still changes mode", async () => {
  localStorage.setItem(storageKey, "hover");
  render(Layout, { props });
  const footer = frame().querySelector<HTMLElement>(".sidebar-control-row")!;
  const button = screen.getByRole("button", { name: "Pin sidebar" });
  expect(button.querySelector('[data-icon="panel-right-open"] rect')).not.toBeNull();
  await fireEvent.pointerEnter(footer, { pointerType: "mouse" });
  await fireEvent.pointerEnter(button, { pointerType: "mouse" });
  await delay();
  expect(folded()).toBe(true);
  await enter();
  expect(folded()).toBe(false);
  await leave();
  await fireEvent.pointerEnter(footer, { pointerType: "mouse" });
  await delay();
  expect(folded()).toBe(true);
  await fireEvent.click(button, { detail: 1 });
  expect(folded()).toBe(false);
  expect(button.querySelector('[data-icon="panel-right-close"] rect')).not.toBeNull();
});

test("keyboard focus opens the preview and holds it until focus leaves", async () => {
  localStorage.setItem(storageKey, "hover");
  render(Layout, { props });
  screen.getByRole("button", { name: "Pin sidebar" }).focus();
  await tick();
  expect(folded()).toBe(false);
  await enter();
  await leave();
  await delay();
  expect(folded()).toBe(false);
  screen.getByRole("link", { name: "Open Account" }).focus();
  await delay();
  expect(folded()).toBe(true);
});

test("pinning a preview keeps it open after pointer and focus leave", async () => {
  localStorage.setItem(storageKey, "hover");
  render(Layout, { props });
  await enter();
  await fireEvent.click(screen.getByRole("button", { name: "Pin sidebar" }), { detail: 1 });
  await leave();
  await delay();
  expect(folded()).toBe(false);
  expect(localStorage.getItem(storageKey)).toBe("pinned");
});

test("mode survives logout and remount, but an open preview does not", async () => {
  vi.stubGlobal("fetch", vi.fn(async () => Response.json({ status: "logged_out" })));
  localStorage.setItem(storageKey, "hover");
  const view = render(Layout, { props });
  await enter();
  await logout();
  view.unmount();
  render(Layout, { props });
  expect(folded()).toBe(true);
  expect(localStorage.getItem(storageKey)).toBe("hover");
});

test("touch does not hover; mobile header opens temporarily without changing mode", async () => {
  const matchMedia = window.matchMedia.bind(window);
  vi.spyOn(window, "matchMedia").mockImplementation((query) => {
    const result = matchMedia(query);
    Object.defineProperty(result, "matches", { value: query === "(max-width: 760px)" });
    return result;
  });
  localStorage.setItem(storageKey, "hover");
  const view = render(Layout, { props });
  await fireEvent.pointerEnter(frame(), { pointerType: "touch" });
  expect(folded()).toBe(true);
  await fireEvent.click(screen.getByRole("button", { name: "Show sidebar" }));
  expect(folded()).toBe(false);
  expect(view.container.querySelector<HTMLElement>("main")!.inert).toBe(true);
  await fireEvent.pointerLeave(frame(), { pointerType: "touch" });
  await delay();
  expect(folded()).toBe(false);
  await fireEvent.click(screen.getByRole("button", { name: "Hide sidebar" }));
  expect(folded()).toBe(true);
  expect(view.container.querySelector<HTMLElement>("main")!.inert).toBe(false);
  expect(localStorage.getItem(storageKey)).toBe("hover");
});

test("invalid storage falls back to pinned", () => {
  localStorage.setItem(storageKey, "not-a-mode");
  render(Layout, { props });
  expect(folded()).toBe(false);
});

test("unavailable storage does not prevent mode switching", async () => {
  vi.spyOn(window, "localStorage", "get").mockImplementation(() => { throw new DOMException("Blocked", "SecurityError"); });
  render(Layout, { props });
  await fireEvent.click(screen.getByRole("button", { name: "Unpin sidebar" }));
  await delay();
  expect(folded()).toBe(true);
  await fireEvent.click(screen.getByRole("button", { name: "Pin sidebar" }));
  expect(folded()).toBe(false);
});

test("full storage still restores the mode and allows changes", async () => {
  localStorage.setItem(storageKey, "hover");
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new DOMException("Full", "QuotaExceededError"); });
  render(Layout, { props });
  await fireEvent.click(screen.getByRole("button", { name: "Pin sidebar" }));
  expect(folded()).toBe(false);
});
