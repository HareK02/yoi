import {
  EditorSelection,
  EditorState,
  type Extension,
  Prec,
} from "@codemirror/state";
import { Decoration, EditorView, keymap } from "@codemirror/view";

const WORKSPACE_SCHEMA_SUFFIX = "} as WorkspaceConfigSchema";
const fixedWrapperMark = Decoration.mark({
  class: "cm-fixed-schema-wrapper",
});

export type FixedWrapperBounds = {
  bodyFrom: number;
  bodyTo: number;
};

export function fixedWrapperBounds(
  state: EditorState,
): FixedWrapperBounds | null {
  const source = state.doc.toString();
  if (!source.startsWith("{")) return null;
  const sourceWithoutTrailingWhitespace = source.trimEnd();
  if (!sourceWithoutTrailingWhitespace.endsWith(WORKSPACE_SCHEMA_SUFFIX)) {
    return null;
  }
  const bodyTo = sourceWithoutTrailingWhitespace.length -
    WORKSPACE_SCHEMA_SUFFIX.length;
  if (bodyTo < 1) return null;
  return { bodyFrom: 1, bodyTo };
}

function fixedWrapperDecorations(state: EditorState) {
  const bounds = fixedWrapperBounds(state);
  if (!bounds) return Decoration.none;
  return Decoration.set([
    fixedWrapperMark.range(0, bounds.bodyFrom),
    fixedWrapperMark.range(bounds.bodyTo, state.doc.length),
  ]);
}

export function fixedSchemaWrapperExtension(): Extension {
  return [
    EditorView.decorations.of((editor) =>
      fixedWrapperDecorations(editor.state)
    ),
    EditorView.atomicRanges.of((editor) =>
      fixedWrapperDecorations(editor.state)
    ),
    EditorState.transactionFilter.of((transaction) => {
      const bounds = fixedWrapperBounds(transaction.startState);
      if (!bounds || !transaction.docChanged) return transaction;
      let allowed = true;
      transaction.changes.iterChangedRanges((from, to) => {
        if (from < bounds.bodyFrom || to > bounds.bodyTo) allowed = false;
      });
      return allowed ? transaction : [];
    }),
    Prec.highest(
      keymap.of([{
        key: "Mod-a",
        run: (editor) => {
          const bounds = fixedWrapperBounds(editor.state);
          if (!bounds) return false;
          editor.dispatch({
            selection: EditorSelection.range(bounds.bodyFrom, bounds.bodyTo),
          });
          return true;
        },
      }]),
    ),
  ];
}

export function moveSelectionIntoFixedWrapper(editor: EditorView) {
  const bounds = fixedWrapperBounds(editor.state);
  if (!bounds) return;
  const selectionInsideBody = editor.state.selection.ranges.every((range) =>
    range.from >= bounds.bodyFrom && range.to <= bounds.bodyTo
  );
  if (!selectionInsideBody) {
    editor.dispatch({
      selection: EditorSelection.cursor(bounds.bodyFrom),
      filter: false,
    });
  }
}
