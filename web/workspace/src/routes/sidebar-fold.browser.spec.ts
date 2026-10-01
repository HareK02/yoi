// @vitest-environment happy-dom

import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { createRawSnippet } from "svelte";
import { logout } from "$lib/workspace/auth/api";
import Layout from "./+layout.svelte";

vi.mock("$app/state", () => ({ page: { url: new URL("https://example.test/") } }));

const storageKey = "yoi.sidebar.folded.v1";
const props = {
  params: {},
  children: createRawSnippet(() => ({ render: () => "<p>Page content</p>" })),
  data: { accessibleWorkspaces: [], workspaceCatalogError: null },
};

beforeEach(() => localStorage.clear());
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  localStorage.clear();
});

test("sidebar starts unfolded and persists both desktop and mobile toggles", async () => {
  const view = render(Layout, { props });
  expect(screen.getByRole("button", { name: "Fold sidebar" }).getAttribute("aria-expanded")).toBe("true");
  const content = view.container.querySelector<HTMLElement>(".sidebar-frame-content")!;
  expect(content.inert).toBe(false);
  await fireEvent.click(screen.getByRole("button", { name: "Fold sidebar" }));
  expect(view.container.querySelector(".sidebar-frame-content")).toBe(content);
  expect(content.inert).toBe(true);
  expect(content.getAttribute("aria-hidden")).toBe("true");
  expect(localStorage.getItem(storageKey)).toBe("true");
  expect(view.container.querySelector(".app-shell")?.classList.contains("sidebar-open")).toBe(false);
  await fireEvent.click(screen.getByRole("button", { name: "Show sidebar" }));
  expect(content.inert).toBe(false);
  expect(content.getAttribute("aria-hidden")).toBe("false");
  expect(localStorage.getItem(storageKey)).toBe("false");
  expect(screen.getByRole("button", { name: "Fold sidebar" })).not.toBeNull();
  await fireEvent.click(screen.getByRole("button", { name: "Hide sidebar" }));
  expect(localStorage.getItem(storageKey)).toBe("true");
  await fireEvent.click(screen.getByRole("button", { name: "Unfold sidebar" }));
  expect(localStorage.getItem(storageKey)).toBe("false");
});

test.each([true, false])("restores stored folded=%s without overwriting it with the default", (folded) => {
  localStorage.setItem(storageKey, String(folded));
  const writes = vi.spyOn(Storage.prototype, "setItem");
  const view = render(Layout, { props });
  expect(screen.getByRole("button", { name: folded ? "Unfold sidebar" : "Fold sidebar" }).getAttribute("aria-expanded")).toBe(String(!folded));
  expect(view.container.querySelector(".app-shell")?.classList.contains("sidebar-open")).toBe(!folded);
  expect(writes.mock.calls.every(([key, value]) => key !== storageKey || value === String(folded))).toBe(true);
});

test("fold preference survives logout and a fresh app mount", async () => {
  vi.stubGlobal("fetch", vi.fn(async () => Response.json({ status: "logged_out" })));
  const view = render(Layout, { props });
  await fireEvent.click(screen.getByRole("button", { name: "Fold sidebar" }));
  await logout();
  view.unmount();
  render(Layout, { props });
  expect(screen.getByRole("button", { name: "Unfold sidebar" })).not.toBeNull();
  expect(localStorage.getItem(storageKey)).toBe("true");
});

test("invalid storage falls back to unfolded", () => {
  localStorage.setItem(storageKey, "not-a-boolean");
  render(Layout, { props });
  expect(screen.getByRole("button", { name: "Fold sidebar" })).not.toBeNull();
});

test("unavailable storage does not prevent folding", async () => {
  vi.spyOn(window, "localStorage", "get").mockImplementation(() => { throw new DOMException("Blocked", "SecurityError"); });
  render(Layout, { props });
  await fireEvent.click(screen.getByRole("button", { name: "Fold sidebar" }));
  expect(screen.getByRole("button", { name: "Unfold sidebar" })).not.toBeNull();
  await fireEvent.click(screen.getByRole("button", { name: "Show sidebar" }));
  expect(screen.getByRole("button", { name: "Fold sidebar" })).not.toBeNull();
});

test("full storage still restores the saved value and allows toggles", async () => {
  localStorage.setItem(storageKey, "true");
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new DOMException("Full", "QuotaExceededError"); });
  render(Layout, { props });
  await fireEvent.click(screen.getByRole("button", { name: "Unfold sidebar" }));
  expect(screen.getByRole("button", { name: "Fold sidebar" })).not.toBeNull();
});
