// @vitest-environment happy-dom

import { cleanup, fireEvent, render } from "@testing-library/svelte";
import { createRawSnippet } from "svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import SidebarFrame from "./SidebarFrame.svelte";

// Stop happy-dom navigation after the component has handled the bubbling click.
const preventBrowserNavigation = (event: MouseEvent) => event.preventDefault();
beforeEach(() => document.addEventListener("click", preventBrowserNavigation));
afterEach(() => {
  cleanup();
  document.removeEventListener("click", preventBrowserNavigation);
});

function mount(mobile = true, mode: "pinned" | "hover" = "pinned") {
  const onOpenChange = vi.fn();
  const onModeChange = vi.fn();
  const children = createRawSnippet(() => ({
    render: () => `<nav>
      <a href="/w/example"><span>Workspace home</span></a>
      <a href="${location.href}">Current page</a>
      <a href="#section">Section</a>
      <a href="/account" target="_blank">New tab</a>
      <a href="/export" download>Download</a>
      <button type="button">Expand group</button>
    </nav>`,
  }));
  return { ...render(SidebarFrame, {
    mobile, mode, open: true, children, onOpenChange, onModeChange,
  }), onOpenChange, onModeChange };
}

test.each(["pinned", "hover"] as const)("mobile %s closes on nested, current-page and hash links", async (mode) => {
  const view = mount(true, mode);
  for (const label of ["Workspace home", "Current page", "Section"]) {
    view.onOpenChange.mockClear();
    await fireEvent.click(view.getByText(label));
    expect(view.onOpenChange).toHaveBeenCalledExactlyOnceWith(false);
    expect(view.onModeChange).not.toHaveBeenCalled();
  }
});

test("mobile ignores disclosure buttons, modified clicks, new tabs, downloads and cancelled navigation", async () => {
  const view = mount();
  for (const label of ["Expand group", "New tab", "Download"]) {
    await fireEvent.click(view.getByText(label));
  }
  const link = view.getByText("Workspace home");
  for (const options of [{ ctrlKey: true }, { metaKey: true }, { shiftKey: true }, { altKey: true }, { button: 1 }]) {
    await fireEvent.click(link, options);
  }
  link.addEventListener("click", (event) => event.preventDefault(), { once: true });
  await fireEvent.click(link);
  expect(view.onOpenChange).not.toHaveBeenCalled();
});

test.each(["pinned", "hover"] as const)("desktop %s navigation does not change sidebar mode or open state", async (mode) => {
  const view = mount(false, mode);
  await fireEvent.click(view.getByText("Workspace home"));
  expect(view.onOpenChange).not.toHaveBeenCalled();
  expect(view.onModeChange).not.toHaveBeenCalled();
});
