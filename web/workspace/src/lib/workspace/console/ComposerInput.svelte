<script lang="ts">
  import {
    Compartment,
    EditorSelection,
    EditorState,
    Prec,
    StateEffect,
    StateField,
  } from "@codemirror/state";
  import {
    Decoration,
    EditorView,
    keymap,
    WidgetType,
    type DecorationSet,
  } from "@codemirror/view";
  import {
    defaultKeymap,
    history,
    historyKeymap,
    isolateHistory,
  } from "@codemirror/commands";
  import { onMount } from "svelte";
  import type { Segment } from "$lib/generated/protocol.ts";
  import {
    handleComposerPaste,
    type ComposerPasteMeasurement,
  } from "$lib/workspace/console/composer-paste.ts";
  import {
    composerPasteAtoms,
    composerPasteToken,
    pasteChipLabel,
    snapshotComposerDraft,
    type ComposerDraftSnapshot,
    type ComposerPaste,
  } from "$lib/workspace/console/composer-draft.ts";
  import { shouldSubmitChatKey } from "$lib/workspace/console/chat-submit.ts";

  interface Props {
    disabled?: boolean;
    ariaLabel?: string;
    ariaKeyShortcuts?: string;
    onchange?: (snapshot: ComposerDraftSnapshot) => void;
    onkeydown?: (event: KeyboardEvent) => void;
    onsubmit?: () => void;
  }

  let {
    disabled = false,
    ariaLabel = "Message",
    ariaKeyShortcuts = "Meta+Enter Control+Enter",
    onchange,
    onkeydown,
    onsubmit,
  }: Props = $props();

  let mountElement: HTMLDivElement;
  let view: EditorView | null = null;
  let nextPasteId = 1;
  let nextPasteKey = 1;
  const editable = new Compartment();

  const registerPaste = StateEffect.define<{ key: number; paste: ComposerPaste }>();
  const pasteRegistry = StateField.define<ReadonlyMap<number, ComposerPaste>>({
    create: () => new Map(),
    update(registry, transaction) {
      const additions = transaction.effects.filter((effect) => effect.is(registerPaste));
      if (additions.length === 0) return registry;
      const next = new Map(registry);
      for (const addition of additions) {
        next.set(addition.value.key, addition.value.paste);
      }
      return next;
    },
  });

  class PasteChipWidget extends WidgetType {
    readonly paste: ComposerPaste;

    constructor(paste: ComposerPaste) {
      super();
      this.paste = paste;
    }

    override eq(other: PasteChipWidget): boolean {
      return other.paste.id === this.paste.id &&
        other.paste.content === this.paste.content &&
        other.paste.chars === this.paste.chars &&
        other.paste.lines === this.paste.lines;
    }

    override toDOM(): HTMLElement {
      const chip = document.createElement("span");
      const label = pasteChipLabel(this.paste);
      chip.className = "composer-paste-chip";
      chip.textContent = label;
      chip.title = label;
      chip.setAttribute("role", "note");
      chip.setAttribute("aria-label", label);
      return chip;
    }

    override ignoreEvent(): boolean {
      return false;
    }
  }

  function pasteDecorations(state: EditorState): DecorationSet {
    const registry = state.field(pasteRegistry);
    return Decoration.set(
      composerPasteAtoms(state.doc.toString(), registry).map((paste) =>
        Decoration.replace({
          widget: new PasteChipWidget(paste),
          inclusive: false,
        }).range(paste.from, paste.to)
      ),
      true,
    );
  }

  const pasteChips = [
    pasteRegistry,
    EditorView.decorations.of((currentView) => pasteDecorations(currentView.state)),
    EditorView.atomicRanges.of((currentView) => pasteDecorations(currentView.state)),
  ];

  function currentSnapshot(state = view?.state): ComposerDraftSnapshot {
    if (!state) {
      return { document: "", content: "", segments: [], pastes: [] };
    }
    return snapshotComposerDraft(state.doc.toString(), state.field(pasteRegistry));
  }

  function emitChange(): void {
    onchange?.(currentSnapshot());
  }

  function insertPasteChip(content: string, measurement: ComposerPasteMeasurement): void {
    if (!view) return;
    const selection = view.state.selection.main;
    const key = nextPasteKey++;
    const paste: ComposerPaste = {
      id: nextPasteId++,
      content,
      chars: measurement.charCount,
      lines: measurement.logicalLineCount,
    };
    const token = composerPasteToken(key);
    view.dispatch({
      changes: { from: selection.from, to: selection.to, insert: token },
      selection: EditorSelection.cursor(selection.from + token.length),
      effects: registerPaste.of({ key, paste }),
      annotations: isolateHistory.of("full"),
      userEvent: "input.paste",
    });
  }

  function selectedClipboardContent(state: EditorState): string | null {
    const selection = state.selection.main;
    if (selection.empty) return null;
    const document = state.doc.sliceString(selection.from, selection.to);
    const registry = state.field(pasteRegistry);
    const selectedRegistry = new Map<number, ComposerPaste>();
    for (const atom of composerPasteAtoms(state.doc.toString(), registry)) {
      if (atom.from >= selection.from && atom.to <= selection.to) {
        selectedRegistry.set(atom.key, atom);
      }
    }
    return snapshotComposerDraft(document, selectedRegistry).content;
  }

  function deleteAdjacentPasteFromView(
    currentView: EditorView,
    direction: "backward" | "forward",
  ): boolean {
    const selection = currentView.state.selection.main;
    if (!selection.empty) return false;
    const pastes = composerPasteAtoms(
      currentView.state.doc.toString(),
      currentView.state.field(pasteRegistry),
    );
    const paste = direction === "backward"
      ? pastes.find((candidate) => candidate.to === selection.head)
      : pastes.find((candidate) => candidate.from === selection.head);
    if (!paste) return false;
    currentView.dispatch({
      changes: { from: paste.from, to: paste.to },
      selection: EditorSelection.cursor(paste.from),
      annotations: isolateHistory.of("full"),
      userEvent: "delete",
    });
    return true;
  }

  onMount(() => {
    view = new EditorView({
      parent: mountElement,
      state: EditorState.create({
        extensions: [
          history(),
          Prec.highest(keymap.of([
            {
              key: "Backspace",
              run: (currentView) =>
                deleteAdjacentPasteFromView(currentView, "backward"),
            },
            {
              key: "Delete",
              run: (currentView) =>
                deleteAdjacentPasteFromView(currentView, "forward"),
            },
          ])),
          keymap.of([...defaultKeymap, ...historyKeymap]),
          pasteChips,
          editable.of(EditorView.editable.of(!disabled)),
          EditorState.allowMultipleSelections.of(false),
          EditorView.lineWrapping,
          EditorView.contentAttributes.of({
            "aria-label": ariaLabel,
            "aria-keyshortcuts": ariaKeyShortcuts,
            "aria-multiline": "true",
            role: "textbox",
            spellcheck: "true",
          }),
          EditorView.updateListener.of((update) => {
            if (update.docChanged || update.transactions.some((tx) => tx.effects.length > 0)) {
              emitChange();
            }
          }),
          Prec.high(EditorView.domEventHandlers({
            paste(event) {
              return handleComposerPaste(event, insertPasteChip);
            },
            copy(event, currentView) {
              const content = selectedClipboardContent(currentView.state);
              if (content === null || !event.clipboardData) return false;
              event.preventDefault();
              event.clipboardData.setData("text/plain", content);
              return true;
            },
            cut(event, currentView) {
              if (disabled) return false;
              const content = selectedClipboardContent(currentView.state);
              if (content === null || !event.clipboardData) return false;
              event.preventDefault();
              event.clipboardData.setData("text/plain", content);
              const selection = currentView.state.selection.main;
              currentView.dispatch({
                changes: { from: selection.from, to: selection.to },
                selection: EditorSelection.cursor(selection.from),
                userEvent: "delete.cut",
              });
              return true;
            },
            keydown(event) {
              onkeydown?.(event);
              if (event.defaultPrevented) return true;
              if (
                shouldSubmitChatKey(event, {
                  mode: "mod-enter",
                  modKey: "auto",
                  enabled: !disabled,
                })
              ) {
                event.preventDefault();
                onsubmit?.();
                return true;
              }
              return false;
            },
          })),
          EditorView.theme({
            "&": { backgroundColor: "transparent" },
            ".cm-scroller": { fontFamily: "inherit" },
            ".cm-content": { caretColor: "var(--text-strong)" },
            "&.cm-focused": { outline: "none" },
          }),
        ],
      }),
    });
    emitChange();

    return () => {
      view?.destroy();
      view = null;
    };
  });

  $effect(() => {
    const isDisabled = disabled;
    view?.dispatch({
      effects: editable.reconfigure(EditorView.editable.of(!isDisabled)),
    });
  });

  export function snapshot(): ComposerDraftSnapshot {
    return currentSnapshot();
  }

  export function focus(): void {
    view?.focus();
  }

  export function containsTarget(target: EventTarget | null): boolean {
    return target instanceof Node && Boolean(view?.dom.contains(target));
  }

  export function cursor(): number {
    return view?.state.selection.main.head ?? 0;
  }

  export function replaceRange(from: number, to: number, content: string): void {
    if (!view) return;
    view.dispatch({
      changes: { from, to, insert: content },
      selection: EditorSelection.cursor(from + content.length),
      userEvent: "input.complete",
    });
  }

  export function clear(): void {
    if (!view) return;
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: "" },
      selection: EditorSelection.cursor(0),
      userEvent: "input",
    });
  }

  export function restoreSegments(segments: readonly Segment[]): void {
    if (!view) return;
    let document = "";
    const effects: StateEffect<{ key: number; paste: ComposerPaste }>[] = [];
    let highestPasteId = nextPasteId - 1;
    for (const segment of segments) {
      if (segment.kind === "text") {
        document += segment.content;
      } else if (segment.kind === "paste") {
        const key = nextPasteKey++;
        const paste: ComposerPaste = {
          id: segment.id,
          content: segment.content,
          chars: segment.chars,
          lines: segment.lines,
        };
        highestPasteId = Math.max(highestPasteId, paste.id);
        document += composerPasteToken(key);
        effects.push(registerPaste.of({ key, paste }));
      } else if (segment.kind === "file_ref") {
        document += `@${segment.path}`;
      }
    }
    nextPasteId = highestPasteId + 1;
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: document },
      selection: EditorSelection.cursor(document.length),
      effects,
      userEvent: "input.restore",
    });
  }
</script>

<div class="composer-input" class:disabled bind:this={mountElement}></div>

<style>
  .composer-input {
    min-width: 0;
    flex: 1;
    color: var(--text-strong);
    font: inherit;
  }

  .composer-input.disabled {
    opacity: 0.56;
  }

  .composer-input :global(.cm-editor) {
    min-height: 5.35rem;
    max-height: 10rem;
  }

  .composer-input :global(.cm-scroller) {
    overflow-y: auto;
  }

  .composer-input :global(.cm-content) {
    min-height: 5.35rem;
    padding: 0.55rem 3.4rem 3rem 0.65rem;
    line-height: 1.45;
  }

  .composer-input :global(.cm-line) {
    padding: 0;
  }

  .composer-input :global(.composer-paste-chip) {
    display: inline-flex;
    align-items: center;
    max-width: min(26rem, 70vw);
    margin: 0 0.15rem;
    padding: 0.08rem 0.42rem;
    overflow: hidden;
    border: 1px solid color-mix(in srgb, var(--accent) 42%, var(--line));
    border-radius: 999px;
    background: color-mix(in srgb, var(--accent) 10%, var(--bg-subtle));
    color: var(--text-muted);
    font-size: 0.72rem;
    font-weight: 600;
    line-height: 1.35;
    text-overflow: ellipsis;
    vertical-align: baseline;
    white-space: nowrap;
  }
</style>
