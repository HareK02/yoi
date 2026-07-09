<script lang="ts">
  import { EditorState } from '@codemirror/state';
  import { EditorView, keymap, lineNumbers, highlightActiveLine, drawSelection } from '@codemirror/view';
  import { decodal } from 'decodal-codemirror';

  let {
    value = '',
    readonly = false,
    ariaLabel = 'Decodal source',
    onChange = (_value: string) => {},
  }: {
    value?: string;
    readonly?: boolean;
    ariaLabel?: string;
    onChange?: (value: string) => void;
  } = $props();

  let host = $state<HTMLDivElement | null>(null);
  let view = $state<EditorView | null>(null);

  const theme = EditorView.theme({
    '&': {
      border: '1px solid var(--border-subtle)',
      borderRadius: '0.75rem',
      minHeight: '24rem',
      background: 'var(--surface-2)',
      color: 'var(--text-primary)',
      fontSize: '0.9rem',
    },
    '.cm-scroller': { fontFamily: 'var(--font-mono)', minHeight: '24rem' },
    '.cm-content': { padding: '0.75rem 0' },
    '.cm-gutters': { background: 'var(--surface-2)', color: 'var(--text-muted)' },
    '.cm-activeLine': { backgroundColor: 'rgba(125, 211, 252, 0.08)' },
  });

  $effect(() => {
    if (!host || view) return;
    const editor = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: value,
        extensions: [
          lineNumbers(),
          drawSelection(),
          highlightActiveLine(),
          decodal(),
          keymap.of([]),
          EditorState.readOnly.of(readonly),
          EditorView.editable.of(!readonly),
          EditorView.updateListener.of((update) => {
            if (update.docChanged) onChange(update.state.doc.toString());
          }),
          theme,
        ],
      }),
    });
    view = editor;
    return () => {
      editor.destroy();
      view = null;
    };
  });

  $effect(() => {
    if (!view) return;
    const current = view.state.doc.toString();
    if (current !== value) {
      view.dispatch({ changes: { from: 0, to: current.length, insert: value } });
    }
  });
</script>

<div bind:this={host} role="textbox" aria-label={ariaLabel} aria-readonly={readonly}></div>
