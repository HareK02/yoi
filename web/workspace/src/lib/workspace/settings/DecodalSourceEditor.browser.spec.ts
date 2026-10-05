// @vitest-environment happy-dom

import {
  type CompletionResult,
  completionStatus,
  startCompletion,
} from "@codemirror/autocomplete";
import { EditorView } from "@codemirror/view";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import DecodalSourceEditor from "./DecodalSourceEditor.svelte";

afterEach(cleanup);

function setup(
  value: string,
  props: {
    readonly?: boolean;
    fixedSchemaWrapper?: boolean;
    onComplete?: (
      source: string,
      offset: number,
      explicit: boolean,
    ) => Promise<CompletionResult | null>;
  } = {},
) {
  const onChange = vi.fn();
  const result = render(DecodalSourceEditor, { value, onChange, ...props });
  const content = result.container.querySelector<HTMLElement>(".cm-content")!;
  const editor = EditorView.findFromDOM(content)!;
  editor.focus();
  return { ...result, content, editor, onChange };
}

test.each([
  { name: "plain line", value: "name = 1", anchor: 8, expected: "name = 1\n" },
  {
    name: "object body",
    value: "{\n  name = 1",
    anchor: 12,
    expected: "{\n  name = 1\n  ",
  },
  { name: "empty object", value: "{}", anchor: 1, expected: "{\n  \n}" },
  {
    name: "nested object",
    value: "{\n  nested = {}\n}",
    anchor: 14,
    expected: "{\n  nested = {\n    \n  }\n}",
  },
])(
  "Enter inserts an indented newline in $name",
  async ({ value, anchor, expected }) => {
    const { content, editor, onChange } = setup(value);
    editor.dispatch({ selection: { anchor } });
    expect(await fireEvent.keyDown(content, { key: "Enter", code: "Enter" }))
      .toBe(false);
    expect(editor.state.doc.toString()).toBe(expected);
    expect(onChange).toHaveBeenLastCalledWith(expected);
  },
);

test("Enter indents inside the fixed wrapper without altering its assertion", async () => {
  const { content, editor } = setup("{} as WorkspaceConfigSchema\n", {
    fixedSchemaWrapper: true,
  });
  await fireEvent.keyDown(content, { key: "Enter", code: "Enter" });
  expect(editor.state.doc.toString()).toBe(
    "{\n  \n} as WorkspaceConfigSchema\n",
  );
  expect(editor.state.selection.main.head).toBe(4);
});

test("Enter does not mutate readonly sources", async () => {
  const value = "{\n  name = 1";
  const { content, editor, onChange } = setup(value, { readonly: true });
  editor.dispatch({ selection: { anchor: value.length } });
  await fireEvent.keyDown(content, { key: "Enter", code: "Enter" });
  expect(editor.state.doc.toString()).toBe(value);
  expect(onChange).not.toHaveBeenCalled();
});

test("Enter adds an indented newline instead of accepting a visible completion", async () => {
  const value = "{\n  na";
  const { content, editor } = setup(value, {
    onComplete: async () => ({
      from: 4,
      options: [{ label: "name", type: "property" }],
    }),
  });
  editor.dispatch({ selection: { anchor: value.length } });
  startCompletion(editor);
  await waitFor(() => expect(completionStatus(editor.state)).toBe("active"));
  // Wait past CodeMirror's completion interaction delay, so accepting a
  // suggestion would be observable rather than ignored as an early keypress.
  await new Promise((resolve) => setTimeout(resolve, 100));
  await fireEvent.keyDown(content, { key: "Enter", code: "Enter" });
  expect(editor.state.doc.toString()).toBe("{\n  na\n  ");
  await waitFor(() => expect(completionStatus(editor.state)).toBeNull());
});

test("Tab indents the current line and Shift+Tab removes indentation", async () => {
  const { content, editor, onChange } = setup("name = 1");
  editor.dispatch({ selection: { anchor: 4 } });
  expect(await fireEvent.keyDown(content, { key: "Tab", code: "Tab" })).toBe(
    false,
  );
  expect(editor.state.doc.toString()).toBe("  name = 1");
  expect(onChange).toHaveBeenLastCalledWith("  name = 1");
  expect(editor.hasFocus).toBe(true);
  await fireEvent.keyDown(content, { key: "Tab", code: "Tab", shiftKey: true });
  expect(editor.state.doc.toString()).toBe("name = 1");
  expect(onChange).toHaveBeenLastCalledWith("name = 1");
});

test("Tab and Shift+Tab indent every selected line", async () => {
  const value = "first = 1\nsecond = 2";
  const { content, editor } = setup(value);
  editor.dispatch({ selection: { anchor: 0, head: value.length } });
  await fireEvent.keyDown(content, { key: "Tab", code: "Tab" });
  expect(editor.state.doc.toString()).toBe("  first = 1\n  second = 2");
  await fireEvent.keyDown(content, { key: "Tab", code: "Tab", shiftKey: true });
  expect(editor.state.doc.toString()).toBe(value);
});

test("Tab inserts indentation on a blank line", async () => {
  const { content, editor } = setup("");
  await fireEvent.keyDown(content, { key: "Tab", code: "Tab" });
  expect(editor.state.doc.toString()).toBe("  ");
});

test("Tab and Shift+Tab do not mutate readonly sources", async () => {
  const value = "  name = 1";
  const { content, editor, onChange } = setup(value, { readonly: true });
  await fireEvent.keyDown(content, { key: "Tab", code: "Tab" });
  await fireEvent.keyDown(content, { key: "Tab", code: "Tab", shiftKey: true });
  expect(editor.state.doc.toString()).toBe(value);
  expect(onChange).not.toHaveBeenCalled();
});

test("Tab indents the schema body without modifying the fixed wrapper", async () => {
  const value = "{\n  name = 1\n} as WorkspaceConfigSchema\n";
  const { content, editor } = setup(value, { fixedSchemaWrapper: true });
  editor.dispatch({ selection: { anchor: value.indexOf("name") } });
  await fireEvent.keyDown(content, { key: "Tab", code: "Tab" });
  expect(editor.state.doc.toString()).toBe(
    "{\n    name = 1\n} as WorkspaceConfigSchema\n",
  );
  await fireEvent.keyDown(content, { key: "Tab", code: "Tab", shiftKey: true });
  expect(editor.state.doc.toString()).toBe(value);
  editor.dispatch({ selection: { anchor: 0 } });
  await fireEvent.keyDown(content, { key: "Tab", code: "Tab" });
  expect(editor.state.doc.toString()).toBe(value);
});
