// @vitest-environment happy-dom

import { cleanup, fireEvent, render, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import ConsoleAttachmentImage from "./ConsoleAttachmentImage.svelte";
import ConsoleLineItem from "./ConsoleLineItem.svelte";
import type { ConsoleLine } from "./model.ts";
import {
  ConsoleAttachmentFetchError,
  loadConsoleAttachment,
} from "./attachment-loader.ts";

vi.mock("./attachment-loader.ts", async (importOriginal) => ({
  ...await importOriginal<typeof import("./attachment-loader.ts")>(),
  loadConsoleAttachment: vi.fn(),
}));

const attachment = {
  attachment_id: "image-a",
  media_type: "image/png",
  byte_len: 3,
};
const blob = new Blob([new Uint8Array([1, 2, 3])], { type: "image/png" });

function installObserver(automaticallyIntersect = true) {
  const observers: { notify: (near: boolean) => void; options?: IntersectionObserverInit; disconnect: ReturnType<typeof vi.fn> }[] = [];
  vi.stubGlobal("IntersectionObserver", class {
    notify: (near: boolean) => void;
    disconnect = vi.fn();
    constructor(callback: IntersectionObserverCallback, options?: IntersectionObserverInit) {
      this.notify = (near: boolean) => callback(
        [{ isIntersecting: near } as IntersectionObserverEntry],
        this as unknown as IntersectionObserver,
      );
      observers.push({ notify: this.notify, options, disconnect: this.disconnect });
    }
    observe() {
      if (automaticallyIntersect) queueMicrotask(() => this.notify(true));
    }
  });
  return observers;
}

beforeEach(() => {
  installObserver();
  vi.mocked(loadConsoleAttachment).mockReset().mockResolvedValue(blob);
  let nextUrl = 0;
  vi.spyOn(URL, "createObjectURL").mockImplementation(() => `blob:image-${++nextUrl}`);
  vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {});
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

test("Overview neither mounts nor loads images, and switching back releases the Normal preview", async () => {
  const observers = installObserver(false);
  const item: ConsoleLine = {
    id: "view-image", kind: "tool", title: "ViewImage", body: "Attached image", source: "event",
    attachments: [attachment],
  };
  const attachmentUrl = vi.fn(() => "/image/a");
  const view = render(ConsoleLineItem, { item, viewMode: "overview", attachmentUrl });
  expect(view.container.querySelector(".console-attachments")).toBeNull();
  expect(attachmentUrl).not.toHaveBeenCalled();
  expect(loadConsoleAttachment).not.toHaveBeenCalled();
  expect(observers).toHaveLength(0);

  await view.rerender({ item, viewMode: "normal", attachmentUrl });
  expect(observers).toHaveLength(1);
  expect(loadConsoleAttachment).not.toHaveBeenCalled();
  observers[0].notify(true);
  await fireEvent.click(await view.findByRole("button", { name: "Enlarge tool output image" }));
  const dialog = view.getByRole("dialog") as HTMLDialogElement;
  await view.rerender({ item, viewMode: "overview", attachmentUrl });
  expect(dialog.open).toBe(false);
  expect(view.container.querySelector(".console-attachments")).toBeNull();
  expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:image-1");
  expect(loadConsoleAttachment).toHaveBeenCalledOnce();
});

test("Normal images wait for the Console scroll viewport and ignore a disposed observer", async () => {
  const observers = installObserver(false);
  const scroll = document.createElement("div");
  scroll.className = "console-scroll";
  document.body.append(scroll);
  try {
    const view = render(ConsoleAttachmentImage, { target: scroll, props: { attachment, url: "/image/a" } });
    expect(observers[0].options).toEqual({ root: scroll, rootMargin: "160px" });
    observers[0].notify(false);
    await Promise.resolve();
    expect(loadConsoleAttachment).not.toHaveBeenCalled();
    await view.rerender({ attachment, url: "/image/b" });
    expect(observers[0].disconnect).toHaveBeenCalled();
    observers[0].notify(true);
    expect(loadConsoleAttachment).not.toHaveBeenCalled();
    observers[1].notify(true);
    await view.findByAltText("Tool output attachment");
    expect(loadConsoleAttachment).toHaveBeenCalledExactlyOnceWith("/image/b");
  } finally {
    cleanup();
    scroll.remove();
  }
});

test("without an observer images require an explicit load instead of eagerly downloading", async () => {
  vi.stubGlobal("IntersectionObserver", undefined);
  const view = render(ConsoleAttachmentImage, { attachment, url: "/image/a" });
  expect(loadConsoleAttachment).not.toHaveBeenCalled();
  await fireEvent.click(view.getByRole("button", { name: "Load image" }));
  await view.findByAltText("Tool output attachment");
  expect(loadConsoleAttachment).toHaveBeenCalledOnce();
});

test("clicking a loaded image opens a modal using the same blob, without another fetch", async () => {
  const view = render(ConsoleAttachmentImage, { attachment, url: "/image/a" });
  const trigger = await view.findByRole("button", { name: "Enlarge tool output image" });
  expect(trigger.getAttribute("type")).toBe("button");
  expect(trigger.getAttribute("aria-haspopup")).toBe("dialog");
  expect(view.queryByRole("dialog")).toBeNull();
  trigger.focus();
  await fireEvent.click(trigger);

  const dialog = view.getByRole("dialog", { name: "Image preview" }) as HTMLDialogElement;
  expect(dialog.open).toBe(true);
  expect(view.getByAltText("Enlarged tool output attachment").getAttribute("src"))
    .toBe(view.getByAltText("Tool output attachment").getAttribute("src"));
  expect(document.activeElement).toBe(view.getByRole("button", { name: "Close" }));
  expect(loadConsoleAttachment).toHaveBeenCalledTimes(1);
  expect(URL.createObjectURL).toHaveBeenCalledTimes(1);

  await fireEvent.click(view.getByAltText("Enlarged tool output attachment"));
  expect(dialog.open).toBe(true);
  await fireEvent.click(view.getByRole("button", { name: "Close" }));
  expect(dialog.open).toBe(false);
  expect(document.activeElement).toBe(trigger);
  expect(URL.revokeObjectURL).not.toHaveBeenCalled();
});

test("Escape, native cancel, and background close the preview and return focus", async () => {
  const view = render(ConsoleAttachmentImage, { attachment, url: "/image/a" });
  const trigger = await view.findByRole("button", { name: "Enlarge tool output image" });
  await fireEvent.click(trigger);
  const dialog = view.getByRole("dialog") as HTMLDialogElement;
  const consoleShortcut = vi.fn();
  window.addEventListener("keydown", consoleShortcut);
  try {
    await fireEvent.keyDown(view.getByRole("button", { name: "Close" }), { key: "Escape" });
    expect(dialog.open).toBe(false);
    expect(consoleShortcut).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(trigger);
  } finally {
    window.removeEventListener("keydown", consoleShortcut);
  }

  await fireEvent.click(trigger);
  await fireEvent(dialog, new Event("cancel", { cancelable: true }));
  expect(dialog.open).toBe(false);
  expect(document.activeElement).toBe(trigger);
  await fireEvent.click(trigger);
  await fireEvent.click(view.getByRole("button", { name: "Dismiss image preview" }));
  expect(dialog.open).toBe(false);
  expect(document.activeElement).toBe(trigger);
  expect(loadConsoleAttachment).toHaveBeenCalledTimes(1);
});

test("unavailable or failed images do not offer a preview; Retry retains existing behavior", async () => {
  const view = render(ConsoleAttachmentImage, { attachment, url: null });
  expect(view.getByText("Image Session is unavailable")).toBeTruthy();
  expect(view.queryByRole("button")).toBeNull();
  vi.mocked(loadConsoleAttachment).mockRejectedValueOnce(new ConsoleAttachmentFetchError("expired", "HTTP 410"));
  await view.rerender({ attachment, url: "/image/a" });
  await view.findByText("Image retention has expired");
  expect(view.queryByRole("button", { name: "Enlarge tool output image" })).toBeNull();
  await fireEvent.click(view.getByRole("button", { name: "Retry" }));
  await view.findByRole("button", { name: "Enlarge tool output image" });
  expect(loadConsoleAttachment).toHaveBeenCalledTimes(2);
});

test("replacing the attachment URL closes its preview and releases the old blob", async () => {
  const view = render(ConsoleAttachmentImage, { attachment, url: "/image/a" });
  await fireEvent.click(await view.findByRole("button", { name: "Enlarge tool output image" }));
  const oldDialog = view.getByRole("dialog") as HTMLDialogElement;
  await view.rerender({ attachment, url: "/image/b" });
  await waitFor(() => expect(view.getByAltText("Tool output attachment").getAttribute("src")).toBe("blob:image-2"));
  expect(oldDialog.open).toBe(false);
  expect(view.queryByRole("dialog")).toBeNull();
  expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:image-1");
  await fireEvent.click(view.getByRole("button", { name: "Enlarge tool output image" }));
  expect(view.getByAltText("Enlarged tool output attachment").getAttribute("src")).toBe("blob:image-2");
  const currentDialog = view.getByRole("dialog") as HTMLDialogElement;
  view.unmount();
  expect(currentDialog.open).toBe(false);
  expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:image-2");
});

test("a late response from a replaced attachment cannot replace the current preview", async () => {
  let resolveOld!: (value: Blob) => void;
  vi.mocked(loadConsoleAttachment).mockImplementationOnce(() => new Promise((resolve) => resolveOld = resolve));
  const view = render(ConsoleAttachmentImage, { attachment, url: "/image/a" });
  await waitFor(() => expect(loadConsoleAttachment).toHaveBeenCalledWith("/image/a"));
  await view.rerender({ attachment, url: "/image/b" });
  await view.findByRole("button", { name: "Enlarge tool output image" });
  resolveOld(blob);
  await Promise.resolve();
  expect(URL.createObjectURL).toHaveBeenCalledTimes(1);
  expect(view.getByAltText("Tool output attachment").getAttribute("src")).toBe("blob:image-1");
});

test("unmounting during loading does not create a leaked blob or preview", async () => {
  let resolveLoad!: (value: Blob) => void;
  vi.mocked(loadConsoleAttachment).mockImplementationOnce(() => new Promise((resolve) => resolveLoad = resolve));
  const view = render(ConsoleAttachmentImage, { attachment, url: "/image/a" });
  await waitFor(() => expect(loadConsoleAttachment).toHaveBeenCalledOnce());
  view.unmount();
  resolveLoad(blob);
  await Promise.resolve();
  expect(URL.createObjectURL).not.toHaveBeenCalled();
});
