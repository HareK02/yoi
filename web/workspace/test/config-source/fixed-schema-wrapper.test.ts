declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
};

import { EditorState } from "@codemirror/state";
import {
  fixedSchemaWrapperExtension,
  fixedWrapperBounds,
} from "../../src/lib/workspace/config-source/fixed-schema-wrapper.ts";

function assertEquals(actual: unknown, expected: unknown, message: string) {
  if (actual !== expected) {
    throw new Error(
      `${message}: expected ${String(expected)}, got ${String(actual)}`,
    );
  }
}

function wrappedState(source = "{} as WorkspaceConfigSchema\n") {
  return EditorState.create({
    doc: source,
    extensions: [fixedSchemaWrapperExtension()],
  });
}

Deno.test("fixed schema wrapper allows edits only inside the asserted object", () => {
  const state = wrappedState();
  const bounds = fixedWrapperBounds(state);
  if (!bounds) throw new Error("canonical wrapper should be recognized");
  assertEquals(bounds.bodyFrom, 1, "body should start after the opening brace");
  assertEquals(
    bounds.bodyTo,
    1,
    "empty body should end before the closing brace",
  );

  const inserted = state.update({
    changes: { from: bounds.bodyFrom, insert: " profile = {}; " },
  });
  assertEquals(
    inserted.newDoc.toString(),
    "{ profile = {}; } as WorkspaceConfigSchema\n",
    "body insertion should be accepted",
  );

  const prefixEdit = state.update({ changes: { from: 0, to: 1, insert: "[" } });
  assertEquals(
    prefixEdit.newDoc.toString(),
    state.doc.toString(),
    "opening wrapper edit should be rejected",
  );

  const suffixEdit = state.update({
    changes: { from: bounds.bodyTo, to: bounds.bodyTo + 1, insert: "]" },
  });
  assertEquals(
    suffixEdit.newDoc.toString(),
    state.doc.toString(),
    "schema assertion edit should be rejected",
  );

  const replaceAll = state.update({
    changes: { from: 0, to: state.doc.length, insert: "{}" },
  });
  assertEquals(
    replaceAll.newDoc.toString(),
    state.doc.toString(),
    "a replacement touching fixed and editable ranges should be rejected atomically",
  );
});

Deno.test("authoritative synchronization can bypass the fixed wrapper filter", () => {
  const state = wrappedState();
  const synchronized = state.update({
    changes: { from: 0, to: state.doc.length, insert: "{}" },
    filter: false,
  });
  assertEquals(
    synchronized.newDoc.toString(),
    "{}",
    "programmatic source replacement should bypass user transaction filters",
  );
  assertEquals(
    fixedWrapperBounds(synchronized.state),
    null,
    "bare source should not expose fixed wrapper ranges",
  );
});
