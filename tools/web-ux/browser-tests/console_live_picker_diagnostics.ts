/// <reference lib="dom" />
import type { Page } from "playwright";

// Playwright's listener subscription enables Chromium interception asynchronously.
// Keep it enabled for the Page lifetime and await the browser acknowledgement
// before any real gesture; otherwise keyboard.press can overtake interception.
export async function prepareNativePicker(page: Page): Promise<void> {
  page.on("filechooser", () => {});
  const session = await page.context().newCDPSession(page);
  await session.send("Page.setInterceptFileChooserDialog", { enabled: true });
}

// Observe native controls without substituting them or synthesizing activation.
export async function observePicker(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const records: unknown[] = [];
    Object.assign(window, { __pickerDiagnostics: records });
    const record = (kind: string, element: Element | null, extra = {}) =>
      records.push({
        kind,
        time: performance.now(),
        active: navigator.userActivation.isActive,
        hasBeenActive: navigator.userActivation.hasBeenActive,
        connected: element?.isConnected,
        tag: element?.tagName,
        className: element?.className,
        editor: document.querySelector(".cm-content")?.textContent,
        focused: document.activeElement?.className,
        ...extra,
      });
    document.addEventListener("keydown", (event) => {
      if (["Enter", "Tab", "Escape"].includes(event.key)) {
        record("keydown", event.target as Element, { key: event.key, trusted: event.isTrusted });
        queueMicrotask(() => record("after-keydown", event.target as Element, { key: event.key }));
      }
    }, true);
    const nativeClick = HTMLInputElement.prototype.click;
    HTMLInputElement.prototype.click = function () {
      if (this.type === "file") record("input.click", this, { disabled: this.disabled });
      nativeClick.call(this);
      if (this.type === "file") record("after-input.click", this);
    };
    document.addEventListener("click", (event) => {
      if ((event.target as HTMLInputElement)?.type === "file") {
        record("input-click-event", event.target as Element, { trusted: event.isTrusted });
      }
    }, true);
    const observer = new MutationObserver((changes) => {
      for (const change of changes) {
        for (
          const [kind, nodes] of [
            ["added", change.addedNodes],
            ["removed", change.removedNodes],
          ] as const
        ) {
          for (const node of Array.from(nodes)) {
            if (
              node instanceof Element && (node.matches("input[type=file], .cm-editor") ||
                node.querySelector("input[type=file], .cm-editor"))
            ) record(kind, node);
          }
        }
      }
    });
    observer.observe(document, { childList: true, subtree: true });
  });
}
export async function pickerDiagnostics(page: Page): Promise<unknown> {
  return await page.evaluate(() =>
    (window as unknown as { __pickerDiagnostics: unknown }).__pickerDiagnostics
  );
}
