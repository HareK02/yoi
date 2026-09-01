<script lang="ts">
  import { untrack } from 'svelte';
  import {
    autocompletion,
    completionKeymap,
    completionStatus,
    startCompletion,
    type CompletionContext,
    type CompletionResult,
  } from '@codemirror/autocomplete';
  import { HighlightStyle, syntaxHighlighting } from '@codemirror/language';
  import { Compartment, EditorState } from '@codemirror/state';
  import { EditorView, keymap, lineNumbers, highlightActiveLine, drawSelection } from '@codemirror/view';
  import { tags } from '@lezer/highlight';
  import { decodal } from 'decodal-codemirror';
  import {
    fixedSchemaWrapperExtension,
    moveSelectionIntoFixedWrapper,
  } from '$lib/workspace/config-source/fixed-schema-wrapper.ts';

  let {
    value = '',
    readonly = false,
    ariaLabel = 'Decodal source',
    fixedSchemaWrapper = false,
    onChange = (_value: string) => {},
    onComplete = undefined,
  }: {
    value?: string;
    readonly?: boolean;
    ariaLabel?: string;
    fixedSchemaWrapper?: boolean;
    onChange?: (value: string) => void;
    onComplete?: (source: string, utf16Offset: number, explicit: boolean) => Promise<CompletionResult | null>;
  } = $props();

  let host = $state<HTMLDivElement | null>(null);
  let view = $state.raw<EditorView | null>(null);
  const readonlyCompartment = new Compartment();
  const fixedSchemaWrapperCompartment = new Compartment();

  const syntaxTheme = HighlightStyle.define([
    { tag: tags.keyword, color: 'var(--accent)', fontWeight: '700' },
    { tag: tags.variableName, color: 'var(--code)' },
    { tag: [tags.bool, tags.number], color: 'var(--warning)' },
    { tag: [tags.string, tags.regexp], color: 'var(--success)' },
    { tag: tags.lineComment, color: 'var(--text-muted)', fontStyle: 'italic' },
    { tag: tags.operator, color: 'var(--danger)' },
    { tag: [tags.brace, tags.squareBracket, tags.paren, tags.punctuation], color: 'var(--accent-muted)' },
  ]);

  const theme = EditorView.theme({
    '&': {
      border: '1px solid var(--line)',
      borderRadius: '0.75rem',
      minHeight: '24rem',
      background: 'var(--bg-raised)',
      color: 'var(--text)',
      fontSize: '0.9rem',
    },
    '&.cm-focused': { outline: '1px solid var(--accent-muted)', outlineOffset: '-1px' },
    '.cm-scroller': { fontFamily: 'var(--font-mono)', minHeight: '24rem' },
    '.cm-content': { padding: '0.75rem 0', caretColor: 'var(--text-strong)' },
    '.cm-fixed-schema-wrapper': {
      color: 'var(--text-muted)',
      backgroundColor: 'var(--interactive-muted)',
      fontWeight: '600',
    },
    '.cm-cursor, .cm-dropCursor': { borderLeftColor: 'var(--text-strong)', borderLeftWidth: '2px' },
    '&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection': {
      backgroundColor: 'var(--interactive-selected)',
    },
    '.cm-gutters': { background: 'var(--bg-raised)', color: 'var(--text-muted)', borderColor: 'var(--line)' },
    '.cm-activeLine, .cm-activeLineGutter': { backgroundColor: 'var(--interactive-hover)' },
    '.cm-tooltip': { background: 'var(--bg-raised)', color: 'var(--text)', borderColor: 'var(--line-strong)' },
    '.cm-tooltip-autocomplete > ul > li[aria-selected]': { background: 'var(--interactive-selected)', color: 'var(--text-strong)' },
  });

  function scheduleCompletion(editor: EditorView) {
    queueMicrotask(() => {
      if (
        editor.hasFocus &&
        !editor.state.facet(EditorState.readOnly) &&
        completionStatus(editor.state) === null
      ) {
        startCompletion(editor);
      }
    });
  }

  $effect(() => {
    if (!host || untrack(() => view)) return;
    const initialValue = untrack(() => value);
    const initialReadonly = untrack(() => readonly);
    const initialFixedSchemaWrapper = untrack(() => fixedSchemaWrapper);
    const handleChange = untrack(() => onChange);
    const handleComplete = untrack(() => onComplete);
    const editor = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: initialValue,
        extensions: [
          lineNumbers(),
          drawSelection(),
          highlightActiveLine(),
          decodal({ highlight: false }),
          syntaxHighlighting(syntaxTheme),
          autocompletion({
            override: [
              async (context: CompletionContext) => {
                if (!handleComplete) return null;
                const doc = context.state.doc.toString();
                return await handleComplete(doc, context.pos, context.explicit);
              },
            ],
          }),
          keymap.of(completionKeymap),
          fixedSchemaWrapperCompartment.of(
            initialFixedSchemaWrapper ? fixedSchemaWrapperExtension() : [],
          ),
          readonlyCompartment.of([
            EditorState.readOnly.of(initialReadonly),
            EditorView.editable.of(!initialReadonly),
          ]),
          EditorView.domEventHandlers({
            focus: (_event, editor) => {
              scheduleCompletion(editor);
              return false;
            },
          }),
          EditorView.updateListener.of((update) => {
            if (update.selectionSet && !update.docChanged) {
              scheduleCompletion(update.view);
            }
            if (update.docChanged) handleChange(update.state.doc.toString());
          }),
          theme,
        ],
      }),
    });
    view = editor;
    if (initialFixedSchemaWrapper) moveSelectionIntoFixedWrapper(editor);
    return () => {
      editor.destroy();
      view = null;
    };
  });

  $effect(() => {
    const editor = view;
    const nextReadonly = readonly;
    if (!editor) return;
    editor.dispatch({
      effects: readonlyCompartment.reconfigure([
        EditorState.readOnly.of(nextReadonly),
        EditorView.editable.of(!nextReadonly),
      ]),
    });
    if (!nextReadonly) scheduleCompletion(editor);
  });

  $effect(() => {
    const editor = view;
    const enabled = fixedSchemaWrapper;
    if (!editor) return;
    editor.dispatch({
      effects: fixedSchemaWrapperCompartment.reconfigure(
        enabled ? fixedSchemaWrapperExtension() : [],
      ),
    });
    if (enabled) moveSelectionIntoFixedWrapper(editor);
  });

  $effect(() => {
    if (!view) return;
    const current = view.state.doc.toString();
    if (current !== value) {
      view.dispatch({
        changes: { from: 0, to: current.length, insert: value },
        filter: false,
      });
      if (fixedSchemaWrapper) moveSelectionIntoFixedWrapper(view);
    }
  });
</script>

<div bind:this={host} role="textbox" aria-label={ariaLabel} aria-readonly={readonly}></div>
