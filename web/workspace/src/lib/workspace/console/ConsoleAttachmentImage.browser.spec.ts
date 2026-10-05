// @vitest-environment happy-dom

import { cleanup, fireEvent, render, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import ConsoleAttachmentImage from "./ConsoleAttachmentImage.svelte";
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

beforeEach(() => {
  // Exercise the loader's no-observer fallback deterministically.
  vi.stubGlobal("IntersectionObserver", undefined);
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
