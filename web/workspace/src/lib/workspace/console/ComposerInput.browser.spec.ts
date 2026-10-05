// @vitest-environment happy-dom
import { cleanup, fireEvent, render, waitFor } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import { EditorView } from "@codemirror/view";
import type { FeatureInvocationDescriptor, Segment } from "#lib/generated/protocol.ts";
import { redo, undo } from "@codemirror/commands";
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

test("synchronous draft and cursor ABA invalidate old Feature completion generations", async () => {
  const requests: Array<ReturnType<typeof deferred<Array<{ value: string; invocation: FeatureInvocationDescriptor }>>>> = [];
  const resolver = vi.fn(() => { const result = deferred<Array<{ value: string; invocation: FeatureInvocationDescriptor }>>(); requests.push(result); return result.promise; });
  const ui = render(ComposerInput, { historyScope: "feature-aba", resolveFeatureCompletions: resolver });
  const cm = editor(ui.container);
  type(cm, "/ru");
  await waitFor(() => expect(requests).toHaveLength(1));
  cm.dispatch({ changes: { from: 2, to: 3, insert: "x" } });
  cm.dispatch({ changes: { from: 2, to: 3, insert: "u" } });
  requests[0].resolve([{ value: "stale", invocation: invocationDescriptor }]);
  await waitFor(() => expect(requests).toHaveLength(2));
  expect(ui.queryByRole("option")).toBeNull();
  cm.dispatch({ selection: { anchor: 0 } });
  cm.dispatch({ selection: { anchor: 3 } });
  requests[1].resolve([{ value: "stale-cursor", invocation: invocationDescriptor }]);
  await waitFor(() => expect(requests).toHaveLength(3));
  expect(ui.queryByRole("option")).toBeNull();
  requests[2].resolve([{ value: "run", invocation: invocationDescriptor }]);
  expect((await ui.findByRole("option")).textContent).toContain("/run");
});

test("restored chip descriptor edits reject same-draft cursor ABA", async () => {
  const request = deferred<Array<{ value: string; invocation: FeatureInvocationDescriptor }>>();
  const resolver = vi.fn(() => request.promise);
  const ui = render(ComposerInput, { historyScope: "edit-cursor-aba", resolveFeatureCompletions: resolver });
  ui.component.restoreSegments([invocationSegment]);
  const cm = editor(ui.container);
  await fireEvent.mouseDown(ui.container.querySelector(".composer-typed-chip")!);
  await waitFor(() => expect(resolver).toHaveBeenCalledOnce());
  cm.dispatch({ selection: { anchor: 0 } });
  cm.dispatch({ selection: { anchor: cm.state.doc.length } });
  request.resolve([{ value: "run", invocation: invocationDescriptor }]);
  await Promise.resolve();
  await Promise.resolve();
  expect(ui.component.snapshot().segments).toEqual([invocationSegment]);
});

test.each(["Backspace", "Delete", "range", "cut", "replace"])("pending upload %s cancels its reservation before late completion and Undo", async (operation) => {
  const oncancelupload = vi.fn();
  const ui = render(ComposerInput, { historyScope: `upload-delete-${operation}`, oncancelupload });
  const cm = editor(ui.container);
  type(cm, "before after", 7);
  const reservation = ui.component.reserveUpload("report.md")!;
  const from = 7, to = cm.state.selection.main.head;
  if (operation === "Backspace" || operation === "Delete") {
    cm.dispatch({ selection: { anchor: operation === "Backspace" ? to : from } });
    await fireEvent.keyDown(cm.contentDOM, { key: operation });
  } else if (operation === "replace") {
    cm.dispatch({ changes: { from, to, insert: "replacement" }, userEvent: "input" });
  } else {
    // happy-dom emits selectionchange synchronously while CodeMirror updates a
    // focused DOM range. Exercise the transaction/cut handler unfocused here;
    // the Chromium suite covers focused keyboard range deletion.
    cm.contentDOM.blur();
    cm.dispatch({ selection: { anchor: from, head: to } });
    if (operation === "cut") await fireEvent.cut(cm.contentDOM, { clipboardData: { setData: vi.fn() } });
    else await fireEvent.keyDown(cm.contentDOM, { key: "Backspace" });
  }
  expect(oncancelupload).toHaveBeenCalledExactlyOnceWith(reservation);
  expect(ui.component.completeUpload(reservation, uploadedSegment)).toBe(false);
  expect(ui.component.snapshot().segments.some((segment) => segment.kind === "unknown" || segment.kind === "uploaded_file")).toBe(false);
  undo(cm);
  expect(ui.component.completeUpload(reservation, uploadedSegment)).toBe(false);
  expect(ui.component.snapshot().segments.some((segment) => segment.kind === "unknown" || segment.kind === "uploaded_file")).toBe(false);
  redo(cm);
  ui.component.clear();
  expect(oncancelupload).toHaveBeenCalledOnce();
});

test.each(["clear", "restore", "unmount"])("pending upload %s boundary cancels once without acquiring late staged resources", (boundary) => {
  const oncancelupload = vi.fn();
  const onremoveatom = vi.fn();
  const ui = render(ComposerInput, { historyScope: `upload-boundary-${boundary}`, oncancelupload, onremoveatom });
  const reservation = ui.component.reserveUpload("report.md")!;
  const complete = ui.component.completeUpload;
  if (boundary === "clear") ui.component.clear();
  else if (boundary === "restore") ui.component.restoreSegments([invocationSegment]);
  else ui.unmount();
  expect(oncancelupload).toHaveBeenCalledExactlyOnceWith(reservation);
  expect(complete(reservation, uploadedSegment)).toBe(false);
  expect(onremoveatom).not.toHaveBeenCalled();
});

test("cancelled upload cannot become sendable via late completion or Undo", async () => {
  const onremoveatom = vi.fn();
  const ui = render(ComposerInput, { historyScope: "upload-cancel-late", onremoveatom });
  const cm = editor(ui.container);
  type(cm, "before after", 7);
  const reservation = ui.component.reserveUpload("report.md")!;
  ui.component.cancelUpload(reservation);
  expect(ui.component.completeUpload(reservation, uploadedSegment)).toBe(false);
  undo(cm);
  expect(ui.component.snapshot().segments.some((segment) => segment.kind === "uploaded_file" || segment.kind === "unknown")).toBe(false);
  ui.component.clear();
  expect(onremoveatom).not.toHaveBeenCalled();
});

const invocationDescriptor: FeatureInvocationDescriptor = {
  identity: "feature:test/run", name: "run", aliases: [], display_name: "Run",
  description: "Declared invocation", syntax: "parenthesized",
  arguments: [{ name: "path", position: 0, required: true,
    value_type: { kind: "worker_file" }, completion: { kind: "worker_file" } }],
};
const invocationSegment: Segment = {
  kind: "feature_invoke", invocation: { invocation_id: "invoke-1",
    identity: invocationDescriptor.identity, name: "run",
    arguments: [{ name: "path", value: { kind: "string", value: "a b/資料" } }] },
};
const uploadedSegment: Segment = { kind: "uploaded_file", file: {
  artifact_id: "artifact-1", file_name: "report.md", media_type: "text/markdown",
  created_at_ms: 1, availability: "available", byte_len: 4, sha256: "abcd",
} };

test("argument popup stays anchored to the edited outer call when its data contains a call-looking slash", async () => {
  const descriptor: FeatureInvocationDescriptor = { ...invocationDescriptor, arguments: [...invocationDescriptor.arguments,
    { name: "mode", required: false, value_type: { kind: "enum", values: ["safe"] }, completion: { kind: "static", values: ["safe"] } }] };
  const resolveFeatureArgumentCompletions = vi.fn(async () => [{ value: "completed" }]);
  const ui = render(ComposerInput, { historyScope: "quoted-data-argument", resolveFeatureArgumentCompletions,
    resolveFeatureCompletions: async () => [{ value: "run", invocation: descriptor }] });
  const segment: Segment = { kind: "feature_invoke", invocation: { ...invocationSegment.invocation, arguments: [
    { name: "path", value: { kind: "string", value: "a /run(pa" } },
    { name: "mode", value: { kind: "string", value: "safe" } },
  ] } };
  ui.component.restoreSegments([segment]);
  await fireEvent.mouseDown(ui.container.querySelector(".composer-typed-chip")!);
  const cm = editor(ui.container);
  await waitFor(() => expect(cm.state.doc.toString()).toBe('/run(path="a /run(pa", mode="safe")'));
  cm.dispatch({ selection: { anchor: cm.state.doc.toString().lastIndexOf("(pa") + 3 } });
  await waitFor(() => expect(resolveFeatureArgumentCompletions).toHaveBeenLastCalledWith(
    { invocation: descriptor.identity, argument: "path" }, "a /run(pa", expect.any(AbortSignal),
  ));
  await ui.findByRole("option");
  await fireEvent.keyDown(cm.contentDOM, { key: "Enter" });
  expect(cm.state.doc.toString()).toBe('/run(path="completed", mode="safe")');
});

test("mid-cursor completion of an incomplete quote preserves following arguments and prose", async () => {
  const descriptor: FeatureInvocationDescriptor = { ...invocationDescriptor, arguments: [...invocationDescriptor.arguments,
    { name: "mode", required: false, value_type: { kind: "enum", values: ["safe"] }, completion: { kind: "static", values: ["safe"] } }] };
  const ui = render(ComposerInput, { historyScope: "incomplete-quote-suffix",
    resolveFeatureCompletions: async () => [{ value: "run", invocation: descriptor }],
    resolveFeatureArgumentCompletions: async () => [{ value: "completed" }] });
  const cm = editor(ui.container);
  type(cm, "/ru");
  await ui.findByRole("option");
  await fireEvent.keyDown(cm.contentDOM, { key: "Enter" });
  const value = '/run(path="ab, mode=safe) following prose';
  cm.dispatch({ changes: { from: cm.state.doc.length, insert: value.slice(5) }, selection: { anchor: value.indexOf("ab") + 2 } });
  await ui.findByRole("option");
  await fireEvent.keyDown(cm.contentDOM, { key: "Enter" });
  expect(cm.state.doc.toString()).toBe('/run(path="completed", mode=safe) following prose');
  const snapshot = ui.component.snapshot();
  expect(snapshot.segments[0]).toMatchObject({ kind: "feature_invoke", invocation: { arguments: [
    { name: "path", value: { kind: "string", value: "completed" } },
    { name: "mode", value: { kind: "string", value: "safe" } },
  ] } });
  expect(snapshot.segments[1]).toEqual({ kind: "text", content: " following prose" });
});

test("selected malformed surrogate strings stay editable and reject Submit snapshot", async () => {
  const ui = render(ComposerInput, { historyScope: "invalid-surrogate-submit",
    resolveFeatureCompletions: async () => [{ value: "run", invocation: invocationDescriptor }] });
  const cm = editor(ui.container);
  type(cm, "/ru");
  await ui.findByRole("option");
  await fireEvent.keyDown(cm.contentDOM, { key: "Enter" });
  const invalid = String.raw`"\uD800")`;
  cm.dispatch({ changes: { from: cm.state.doc.length, insert: invalid }, selection: { anchor: 5 + invalid.length } });
  await Promise.resolve();
  expect(ui.container.querySelector(".composer-typed-chip")).toBeNull();
  expect(() => ui.component.snapshot()).toThrow("unpaired Unicode surrogates");
  const valid = String.raw`"\uD83D\uDE00")`;
  cm.dispatch({ changes: { from: 5, to: cm.state.doc.length, insert: valid }, selection: { anchor: 5 + valid.length } });
  const snapshot = ui.component.snapshot();
  expect(snapshot.segments[0]).toMatchObject({ kind: "feature_invoke", invocation: { arguments: [
    { name: "path", value: { kind: "string", value: "😀" } },
  ] } });
});

test("selected calls chipify without promoting matching typed or pasted text", async () => {
  const ui = render(ComposerInput, { historyScope: "invocation-select",
    resolveFeatureCompletions: async () => [{ value: "run", invocation: invocationDescriptor }] });
  const cm = editor(ui.container);
  type(cm, "/ru");
  await ui.findByRole("option");
  await fireEvent.keyDown(cm.contentDOM, { key: "Enter" });
  cm.dispatch({ changes: { from: cm.state.doc.length, insert: '"a b/資料")' }, selection: { anchor: '/run("a b/資料")'.length } });
  await waitFor(() => expect(ui.container.querySelectorAll(".composer-typed-chip")).toHaveLength(1));
  cm.dispatch({ changes: { from: cm.state.doc.length, insert: ' /run("literal")' } });
  await Promise.resolve();
  const snapshot = ui.component.snapshot();
  expect(snapshot.segments.map((segment) => segment.kind)).toEqual(["feature_invoke", "text"]);
  expect(snapshot.segments[1]).toEqual({ kind: "text", content: ' /run("literal")' });
  expect(snapshot.segments[0].kind === "feature_invoke" && snapshot.segments[0].invocation.arguments[0].value)
    .toEqual({ kind: "string", value: "a b/資料" });
});

test("restored invocation chips fetch declarative descriptors and stay editable inside arguments", async () => {
  const resolveFeatureCompletions = vi.fn(async () => [{ value: "run", invocation: invocationDescriptor }]);
  const ui = render(ComposerInput, { historyScope: "invocation-edit", resolveFeatureCompletions });
  ui.component.restoreSegments([invocationSegment]);
  const cm = editor(ui.container);
  await fireEvent.mouseDown(ui.container.querySelector(".composer-typed-chip")!);
  await waitFor(() => expect(cm.state.doc.toString()).toBe('/run(path="a b/資料")'));
  expect(resolveFeatureCompletions).toHaveBeenCalledWith("run", expect.any(AbortSignal));
  cm.dispatch({ changes: { from: 11, to: 12, insert: "c" }, selection: { anchor: 12 } });
  await Promise.resolve();
  expect(ui.container.querySelector(".composer-typed-chip")).toBeNull();
  const snapshot = ui.component.snapshot();
  expect(snapshot.segments[0].kind).toBe("feature_invoke");
});

test("attachment adapter uses descriptor capability rather than a hardcoded slash name", async () => {
  const onclientadapter = vi.fn();
  const adapter = { ...invocationDescriptor, name: "send-file", arguments: [], client_adapter: "attachment" as const };
  const ui = render(ComposerInput, { historyScope: "invocation-adapter", onclientadapter,
    resolveFeatureCompletions: async () => [{ value: adapter.name, invocation: adapter }] });
  const cm = editor(ui.container);
  type(cm, "before /send");
  await ui.findByRole("option");
  await fireEvent.keyDown(cm.contentDOM, { key: "Enter" });
  expect(onclientadapter).toHaveBeenCalledWith(adapter);
  expect(cm.state.doc.toString()).toBe("before ");
});

test("uploads preserve reserved order, deletion undo, cleanup and accepted ownership", async () => {
  const onremoveatom = vi.fn();
  const ui = render(ComposerInput, { historyScope: "invocation-upload", onremoveatom });
  const cm = editor(ui.container);
  type(cm, "before ");
  const first = ui.component.reserveUpload("report.md")!;
  const second = ui.component.reserveUpload("second.md")!;
  cm.dispatch({ changes: { from: cm.state.doc.length, insert: " after" } });
  const secondSegment = { ...uploadedSegment, file: { ...uploadedSegment.file, artifact_id: "artifact-2", file_name: "second.md" } };
  expect(ui.component.completeUpload(second, secondSegment)).toBe(true);
  expect(ui.component.completeUpload(first, uploadedSegment)).toBe(true);
  expect(ui.component.snapshot().segments).toEqual([
    { kind: "text", content: "before " }, uploadedSegment, secondSegment, { kind: "text", content: " after" },
  ]);
  const tokenEnd = "before ".length + 3;
  cm.dispatch({ selection: { anchor: tokenEnd } });
  await fireEvent.keyDown(cm.contentDOM, { key: "Backspace" });
  expect(onremoveatom).not.toHaveBeenCalled();
  undo(cm);
  expect(ui.component.snapshot().segments).toContainEqual(uploadedSegment);
  ui.component.clear(true);
  expect(onremoveatom).not.toHaveBeenCalled();
  expect(undo(cm)).toBe(false);
  ui.component.insertSegment(uploadedSegment, true);
  ui.component.restoreSegments([invocationSegment]);
  expect(onremoveatom).toHaveBeenCalledWith(uploadedSegment);
});

test("restored opaque segments and typed clipboard labels are not lost", async () => {
  const ui = render(ComposerInput, { historyScope: "invocation-restore" });
  const segments: Segment[] = [invocationSegment, uploadedSegment, { kind: "file_ref", path: "a b/c" }, { kind: "flow", selector: "legacy" }];
  ui.component.restoreSegments(segments);
  expect(ui.component.snapshot().segments).toEqual(segments);
  const cm = editor(ui.container);
  cm.dispatch({ selection: { anchor: 0, head: cm.state.doc.length } });
  const setData = vi.fn();
  await fireEvent.copy(cm.contentDOM, { clipboardData: { setData } });
  expect(setData).toHaveBeenCalledWith("text/plain", expect.stringContaining('/run(path="a b/資料")'));
  expect(setData.mock.calls[0][1]).not.toContain("\uFFF8");
});

test("restored descriptor requests cannot edit a newer draft after a scope change", async () => {
  const request = deferred<Array<{ value: string; invocation: FeatureInvocationDescriptor }>>();
  const resolver = vi.fn(() => request.promise);
  const ui = render(ComposerInput, { historyScope: "invocation-stale-edit", completionScope: "worker-a", resolveFeatureCompletions: resolver });
  ui.component.restoreSegments([invocationSegment]);
  await fireEvent.mouseDown(ui.container.querySelector(".composer-typed-chip")!);
  await waitFor(() => expect(resolver).toHaveBeenCalledOnce());
  await ui.rerender({ completionScope: "worker-b" });
  ui.component.restoreSegments([{ kind: "text", content: "new draft" }]);
  request.resolve([{ value: "run", invocation: invocationDescriptor }]);
  await Promise.resolve();
  await Promise.resolve();
  expect(ui.component.snapshot().segments).toEqual([{ kind: "text", content: "new draft" }]);
});

test("pasted invocation text remains literal after descriptor selection", async () => {
  const ui = render(ComposerInput, { historyScope: "invocation-paste-literal", resolveFeatureCompletions: async () => [{ value: "run", invocation: invocationDescriptor }] });
  const cm = editor(ui.container);
  type(cm, "/ru");
  await ui.findByRole("option");
  await fireEvent.keyDown(cm.contentDOM, { key: "Enter" });
  cm.dispatch({ changes: { from: cm.state.doc.length, insert: '"x")' }, selection: { anchor: '/run("x")'.length } });
  await waitFor(() => expect(ui.container.querySelectorAll(".composer-typed-chip")).toHaveLength(1));
  await fireEvent.paste(cm.contentDOM, { clipboardData: { getData: () => ' /run("pasted")', items: [] } });
  expect(ui.component.snapshot().segments.map((segment) => segment.kind)).toEqual(["feature_invoke", "text"]);
});
