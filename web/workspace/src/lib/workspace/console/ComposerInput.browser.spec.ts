// @vitest-environment happy-dom
import { cleanup, fireEvent, render, waitFor } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import { EditorView } from "@codemirror/view";
import ComposerInput from "./ComposerInput.svelte";

afterEach(cleanup);
function editor(container: HTMLElement) {
  return EditorView.findFromDOM(
    container.querySelector(".cm-editor") as HTMLElement,
  )!;
}
function type(view: EditorView, value: string, cursor = value.length) {
  view.focus();
  view.dispatch({
    changes: { from: 0, to: view.state.doc.length, insert: value },
    selection: { anchor: cursor },
  });
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

test("commands auto-open with descriptions; ambiguous Tab does not choose the first; arrows beat history", async () => {
  const oncommand = vi.fn();
  const onsubmit = vi.fn();
  const ui = render(ComposerInput, {
    historyScope: "completion-test",
    oncommand,
    onsubmit,
  });
  const cm = editor(ui.container);
  type(cm, ":");
  await ui.findByRole("listbox");
  expect(ui.getAllByRole("option")).toHaveLength(6);
  expect(ui.getByText("Request immediate Worker context compaction.")).not
    .toBeNull();
  await fireEvent.keyDown(cm.contentDOM, { key: "Tab" });
  expect(cm.state.doc.toString()).toBe(":");
  expect(ui.getByRole("status").textContent).toContain("Select a command");
  await fireEvent.keyDown(cm.contentDOM, { key: "ArrowDown" });
  expect(ui.getAllByRole("option")[0].getAttribute("aria-selected")).toBe(
    "true",
  );
  expect(cm.contentDOM.getAttribute("aria-activedescendant")).toBe(
    ui.getAllByRole("option")[0].id,
  );
  await fireEvent.keyDown(cm.contentDOM, { key: "ArrowUp" });
  expect(ui.getAllByRole("option")[5].getAttribute("aria-selected")).toBe(
    "true",
  );
  await fireEvent.keyDown(cm.contentDOM, { key: "Escape" });
  expect(ui.queryByRole("listbox")).toBeNull();
  expect(cm.state.doc.toString()).toBe(":");
  type(cm, ":comp");
  await ui.findByRole("option");
  await fireEvent.keyDown(cm.contentDOM, { key: "Tab" });
  expect(cm.state.doc.toString()).toBe(":compact ");
  expect(oncommand).not.toHaveBeenCalled();
  await fireEvent.keyDown(cm.contentDOM, { key: "Enter" });
  expect(oncommand).toHaveBeenCalledOnce();
  expect(onsubmit).not.toHaveBeenCalled();
});

test("click selects a command and IME/Shift-Enter do not confirm it", async () => {
  const oncommand = vi.fn();
  const ui = render(ComposerInput, {
    historyScope: "completion-ime",
    oncommand,
  });
  const cm = editor(ui.container);
  type(cm, ":rew");
  await ui.findByRole("option");
  await fireEvent.keyDown(cm.contentDOM, {
    key: "Enter",
    isComposing: true,
    keyCode: 229,
  });
  expect(oncommand).not.toHaveBeenCalled();
  type(cm, ":rew");
  await ui.findByRole("option");
  await fireEvent.keyDown(cm.contentDOM, { key: "Enter", shiftKey: true });
  expect(oncommand).not.toHaveBeenCalled();
  type(cm, ":rew");
  const option = await ui.findByRole("option");
  await fireEvent.pointerDown(option);
  await fireEvent.click(option);
  expect(cm.state.doc.toString()).toBe(":rewind ");
  expect(oncommand).toHaveBeenCalledOnce();
});

test("file popup scrolls selection, drills into directories and Enter accepts without sending", async () => {
  const onsubmit = vi.fn();
  const resolveFileCompletions = vi.fn(async (prefix: string) =>
    prefix === "src/" ? [{ value: "src/main.rs" }] : [
      { value: "src", is_dir: true },
      ...Array.from({ length: 8 }, (_, i) => ({ value: `file-${i}` })),
    ]
  );
  const ui = render(ComposerInput, {
    historyScope: "completion-files",
    onsubmit,
    resolveFileCompletions,
  });
  const cm = editor(ui.container);
  type(cm, "read @");
  await waitFor(() => expect(ui.getAllByRole("option")).toHaveLength(6));
  await fireEvent.keyDown(cm.contentDOM, { key: "ArrowUp" });
  expect(ui.getAllByRole("option")[5].textContent).toContain("file-7");
  await fireEvent.keyDown(cm.contentDOM, { key: "ArrowDown" });
  await fireEvent.keyDown(cm.contentDOM, { key: "Tab" });
  expect(cm.state.doc.toString()).toBe("read @src/");
  await waitFor(() => expect(ui.getAllByRole("option")).toHaveLength(1));
  await fireEvent.keyDown(cm.contentDOM, { key: "Enter" });
  expect(cm.state.doc.toString()).toBe("read @src/main.rs ");
  expect(onsubmit).not.toHaveBeenCalled();
});

test("cursor changes, Escape, scope changes and disable discard late file results", async () => {
  const old = deferred<Array<{ value: string }>>();
  const resolveFileCompletions = vi.fn(() => old.promise);
  const ui = render(ComposerInput, {
    historyScope: "completion-race",
    completionScope: "a",
    resolveFileCompletions,
  });
  const cm = editor(ui.container);
  type(cm, "@old");
  await waitFor(() => expect(resolveFileCompletions).toHaveBeenCalledOnce());
  await fireEvent.keyDown(cm.contentDOM, { key: "Escape" });
  old.resolve([{ value: "old-result" }]);
  await Promise.resolve();
  expect(ui.queryByRole("listbox")).toBeNull();
  type(cm, "@new");
  await ui.findByRole("listbox");
  cm.dispatch({ selection: { anchor: 0 } });
  await waitFor(() => expect(ui.queryByRole("listbox")).toBeNull());
  type(cm, ":comp");
  await ui.findByRole("listbox");
  await ui.rerender({ completionScope: "b" });
  expect(ui.queryByRole("listbox")).toBeNull();
  type(cm, ":rew");
  await ui.findByRole("listbox");
  await ui.rerender({ disabled: true });
  expect(ui.queryByRole("listbox")).toBeNull();
});
