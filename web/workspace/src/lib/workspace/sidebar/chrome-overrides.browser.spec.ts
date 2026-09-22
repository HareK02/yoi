// @vitest-environment happy-dom

import { cleanup, render, waitFor } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import type { Snippet } from "svelte";
import HeaderOverride from "../header/HeaderOverride.svelte";
import SidebarOverride from "./SidebarOverride.svelte";

const mocks = vi.hoisted(() => {
  const disposeHeader = vi.fn();
  const disposeSidebar = vi.fn();
  return {
    disposeHeader,
    disposeSidebar,
    registerHeader: vi.fn(() => disposeHeader),
    registerSidebar: vi.fn(() => disposeSidebar),
  };
});

vi.mock("./context", () => ({
  getSidebarController: () => ({
    registerSidebar: mocks.registerSidebar,
  }),
}));

vi.mock("../header/context", () => ({
  getHeaderController: () => ({
    registerContent: mocks.registerHeader,
  }),
}));

const firstSnippet = (() => {}) as Snippet<[]>;

afterEach(() => {
  cleanup();
  mocks.disposeHeader.mockClear();
  mocks.disposeSidebar.mockClear();
  mocks.registerHeader.mockClear();
  mocks.registerHeader.mockReturnValue(mocks.disposeHeader);
  mocks.registerSidebar.mockClear();
  mocks.registerSidebar.mockReturnValue(mocks.disposeSidebar);
});

test("sidebar override registers only while its route is active", async () => {
  const view = render(SidebarOverride, {
    sidebar: firstSnippet,
    active: false,
  });

  expect(mocks.registerSidebar).not.toHaveBeenCalled();

  await view.rerender({ sidebar: firstSnippet, active: true });
  await waitFor(() => {
    expect(mocks.registerSidebar).toHaveBeenCalledOnce();
    expect(mocks.registerSidebar).toHaveBeenCalledWith(firstSnippet);
  });

  await view.rerender({ sidebar: firstSnippet, active: false });
  await waitFor(() => expect(mocks.disposeSidebar).toHaveBeenCalledOnce());
});

test("header override registers only while its route is active", async () => {
  const view = render(HeaderOverride, {
    content: firstSnippet,
    active: false,
  });

  expect(mocks.registerHeader).not.toHaveBeenCalled();

  await view.rerender({ content: firstSnippet, active: true });
  await waitFor(() => {
    expect(mocks.registerHeader).toHaveBeenCalledOnce();
    expect(mocks.registerHeader).toHaveBeenCalledWith(firstSnippet);
  });

  await view.rerender({ content: firstSnippet, active: false });
  await waitFor(() => expect(mocks.disposeHeader).toHaveBeenCalledOnce());
});
