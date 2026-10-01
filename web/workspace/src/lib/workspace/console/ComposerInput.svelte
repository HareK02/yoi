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
    invertedEffects,
    isolateHistory,
  } from "@codemirror/commands";
  import { onMount } from "svelte";
  import type { Segment } from "#lib/generated/protocol.ts";
  import {
    handleComposerImagePaste,
    measureComposerPaste,
    type ComposerPasteMeasurement,
  } from "#lib/workspace/console/composer-paste.ts";
  import {
    composerDeletionRange,
    composerPasteAtoms,
    composerPasteToken,
    pasteChipLabel,
    snapshotComposerDraft,
    type ComposerDraftSnapshot,
    type ComposerPaste,
    type ComposerTextPaste,
  } from "#lib/workspace/console/composer-draft.ts";
  import {
    ComposerHistory,
    loadComposerHistory,
    saveComposerHistory,
    shouldBrowseComposerHistory,
    type ComposerHistoryDirection,
    type ComposerHistoryEntry,
  } from "#lib/workspace/console/composer-history.ts";
  import { applyCompletion, completionSelection, completionTokenAt, localCommandCompletions, type ComposerCompletionEntry, type ComposerCompletionToken } from "./composer-completion";
  import { shouldSubmitChatKey } from "#lib/workspace/console/chat-submit.ts";

  interface Props {
    disabled?: boolean;
    ariaLabel?: string;
    ariaKeyShortcuts?: string;
    historyScope: string;
    onchange?: (snapshot: ComposerDraftSnapshot) => void;
    onkeydown?: (event: KeyboardEvent) => void;
    onsubmit?: () => void;
    onpasteimages?: (files: File[]) => void;
    oncommand?: () => void;
    completionScope?: string;
    resolveFileCompletions?: (prefix: string, signal: AbortSignal) => Promise<ComposerCompletionEntry[]>;
  }

  let {
    disabled = false,
    ariaLabel = "Message",
    ariaKeyShortcuts = "Meta+Enter Control+Enter",
    historyScope,
    onchange,
    onkeydown,
    onsubmit,
    onpasteimages,
    oncommand,
    completionScope = "",
    resolveFileCompletions,
  }: Props = $props();

  let mountElement: HTMLDivElement;
  let view: EditorView | null = null;
  let composerHistory = new ComposerHistory();
  let restoringHistory = false;
  let nextPasteId = 1;
  let nextPasteKey = 1;
  const editable = new Compartment();

  const completionId = $props.id();
  let completionToken = $state<ComposerCompletionToken | null>(null);
  let completionEntries = $state<ComposerCompletionEntry[]>([]);
  let completionSelected = $state<number | null>(null);
  let completionBusy = $state(false);
  let completionMessage = $state("");
  let completionAbort: AbortController | null = null;
  let completionVersion = 0;
  let completionContext = "";
  const completionStart = $derived(Math.max(0, (completionSelected ?? 0) - 5));
  const visibleCompletions = $derived(completionEntries.slice(completionStart, completionStart + 6));

  function closeCompletion(): void {
    completionVersion++;
    completionAbort?.abort();
    completionAbort = null;
    completionToken = null;
    completionEntries = [];
    completionSelected = null;
    completionBusy = false;
    completionMessage = "";
  }

  function refreshCompletion(force = false): void {
    if (!view) return;
    const selection = view.state.selection.main;
    const document = view.state.doc.toString();
    const context = JSON.stringify([document, selection.from, selection.to]);
    if (!force && context === completionContext) return;
    completionContext = context;
    closeCompletion();
    if (disabled || !view.hasFocus || view.composing || !selection.empty) return;
    const token = completionTokenAt(document, selection.head);
    if (!token) return;
    completionToken = token;
    if (token.kind === "command") {
      completionEntries = localCommandCompletions(token.prefix);
      if (!completionEntries.length) completionMessage = "No matching commands";
      return;
    }
    if (!resolveFileCompletions) { closeCompletion(); return; }
    completionBusy = true;
    const version = completionVersion;
    const abort = new AbortController();
    completionAbort = abort;
    void resolveFileCompletions(token.prefix, abort.signal).then((entries) => {
      if (version !== completionVersion || abort.signal.aborted) return;
      completionEntries = entries;
      completionSelected = entries.length ? 0 : null;
      if (!entries.length) completionMessage = "No matching files";
    }).catch((error) => {
      if (version !== completionVersion || abort.signal.aborted) return;
      completionMessage = error instanceof Error ? error.message : String(error);
    }).finally(() => {
      if (version === completionVersion) completionBusy = false;
    });
  }

  function acceptCompletion(action: "tab" | "accept", index = completionSelected): void {
    if (!view || !completionToken || disabled) return;
    const token = completionToken;
    const document = view.state.doc.toString();
    if (token.kind === "command" && index === null) {
      const exact = completionEntries.findIndex((entry) => entry.value === token.prefix);
      if (exact >= 0) index = exact;
    }
    if (index === null && completionEntries.length === 1) index = 0;
    if (index === null && completionEntries.length > 1) {
      completionMessage = "Select a command with ↑/↓ or keep typing";
      return;
    }
    const entry = index === null ? undefined : completionEntries[index];
    if (!entry) return;
    const result = applyCompletion(document, token, entry, action);
    const to = document.length - (result.value.length - result.cursor);
    closeCompletion();
    view.dispatch({
      changes: { from: token.start, to, insert: result.value.slice(token.start, result.cursor) },
      selection: EditorSelection.cursor(result.cursor), userEvent: "input.complete",
    });
    view.focus();
    if (token.kind === "command" && action === "accept") oncommand?.();
    else queueMicrotask(() => refreshCompletion(true));
  }

  function completionKeydown(event: KeyboardEvent): boolean {
    if (disabled || event.isComposing || view?.composing || event.keyCode === 229) return false;
    if (event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) return false;
    if (event.key === "Escape" && completionToken) {
      event.preventDefault(); closeCompletion(); return true;
    }
    if (completionToken && completionEntries.length && (event.key === "ArrowUp" || event.key === "ArrowDown")) {
      event.preventDefault();
      completionSelected = completionSelection(completionSelected, completionEntries.length, event.key === "ArrowDown" ? 1 : -1);
      completionMessage = "";
      return true;
    }
    if (event.key === "Tab") {
      if (!completionToken) refreshCompletion(true);
      if (!completionToken) return false;
      event.preventDefault(); acceptCompletion("tab"); return true;
    }
    if (event.key === "Enter" && completionToken && completionEntries.length) {
      event.preventDefault(); acceptCompletion("accept"); return true;
    }
    if (event.key === "Enter" && view?.state.doc.toString().trimStart().startsWith(":")) {
      event.preventDefault(); oncommand?.(); return true;
    }
    return false;
  }

  $effect(() => {
    const activeId = completionSelected === null ? undefined : completionId + "-" + completionSelected;
    const open = completionToken !== null;
    if (!view) return;
    view.contentDOM.setAttribute("aria-autocomplete", "list");
    if (open) view.contentDOM.setAttribute("aria-controls", completionId);
    else view.contentDOM.removeAttribute("aria-controls");
    if (open && activeId) view.contentDOM.setAttribute("aria-activedescendant", activeId);
    else view.contentDOM.removeAttribute("aria-activedescendant");
  });

  $effect(() => {
    void completionScope;
    void disabled;
    closeCompletion();
    completionContext = "";
  });

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

  const registerTextPaste = StateEffect.define<ComposerTextPaste>();
  const restoreTextPastes = StateEffect.define<readonly ComposerTextPaste[]>();
  const textPasteState = StateField.define<readonly ComposerTextPaste[]>({
    create: () => [],
    update(textPastes, transaction) {
      const restored = transaction.effects.find((effect) =>
        effect.is(restoreTextPastes)
      );
      if (restored) return restored.value;
      const retained: ComposerTextPaste[] = [];
      for (const paste of textPastes) {
        let touched = false;
        transaction.changes.iterChangedRanges((from, to) => {
          const replacesContent = from < paste.to && to > paste.from;
          const insertsInside = from === to && from > paste.from && from < paste.to;
          if (replacesContent || insertsInside) touched = true;
        });
        if (touched) continue;
        retained.push({
          ...paste,
          from: transaction.changes.mapPos(paste.from, 1),
          to: transaction.changes.mapPos(paste.to, -1),
        });
      }
      for (const effect of transaction.effects) {
        if (effect.is(registerTextPaste)) retained.push(effect.value);
      }
      return retained;
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
    textPasteState,
    invertedEffects.of((transaction) =>
      transaction.docChanged
        ? [restoreTextPastes.of(transaction.startState.field(textPasteState))]
        : []
    ),
    EditorView.decorations.of((currentView) => pasteDecorations(currentView.state)),
    EditorView.atomicRanges.of((currentView) => pasteDecorations(currentView.state)),
  ];

  function currentSnapshot(state = view?.state): ComposerDraftSnapshot {
    if (!state) {
      return { document: "", content: "", segments: [], pastes: [], textPastes: [] };
    }
    return snapshotComposerDraft(
      state.doc.toString(),
      state.field(pasteRegistry),
      state.field(textPasteState),
    );
  }

  function emitChange(): void {
    onchange?.(currentSnapshot());
  }

  function historyEntry(state: EditorState): ComposerHistoryEntry {
    const snapshot = currentSnapshot(state);
    return {
      segments: snapshot.segments,
      preserveExactText: snapshot.textPastes.length > 0,
    };
  }

  function browseHistory(currentView: EditorView, direction: ComposerHistoryDirection): boolean {
    const selection = currentView.state.selection.main;
    const cursorLine = currentView.state.doc.lineAt(selection.head).number;
    if (!shouldBrowseComposerHistory({
      direction,
      cursorLine,
      lineCount: currentView.state.doc.lines,
      selectionEmpty: selection.empty,
      readOnly: currentView.state.readOnly,
      composing: currentView.composing,
    })) return false;

    const entry = direction === "older"
      ? composerHistory.previous(historyEntry(currentView.state))
      : composerHistory.next();
    if (!entry) return false;

    restoringHistory = true;
    try {
      restoreSegments(entry.segments, entry.preserveExactText);
    } finally {
      restoringHistory = false;
    }
    return true;
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

  function insertTextPaste(content: string): void {
    if (!view) return;
    const selection = view.state.selection.main;
    const rendered = view.state.toText(content).toString();
    view.dispatch({
      changes: { from: selection.from, to: selection.to, insert: rendered },
      selection: EditorSelection.cursor(selection.from + rendered.length),
      effects: registerTextPaste.of({
        from: selection.from,
        to: selection.from + rendered.length,
        rendered,
        content,
      }),
      annotations: isolateHistory.of("full"),
      userEvent: "input.paste",
    });
  }

  function handlePasteEvent(event: ClipboardEvent): boolean {
    if (disabled || view?.state.readOnly) return false;
    if (handleComposerImagePaste(event, onpasteimages)) return true;
    const content = event.clipboardData?.getData("text/plain");
    if (!content) return false;
    const measurement = measureComposerPaste(content);
    event.preventDefault();
    if (measurement.presentation === "chip") {
      insertPasteChip(content, measurement);
    } else {
      insertTextPaste(content);
    }
    return true;
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
    const selectedTextPastes = state.field(textPasteState)
      .filter((paste) => paste.from >= selection.from && paste.to <= selection.to)
      .map((paste) => ({
        ...paste,
        from: paste.from - selection.from,
        to: paste.to - selection.from,
      }));
    return snapshotComposerDraft(document, selectedRegistry, selectedTextPastes).content;
  }

  function deleteAdjacentPasteFromView(
    currentView: EditorView,
    direction: "backward" | "forward",
  ): boolean {
    if (currentView.state.readOnly) return false;
    const selection = currentView.state.selection.main;
    const pastes = composerPasteAtoms(
      currentView.state.doc.toString(),
      currentView.state.field(pasteRegistry),
    );
    const deletion = composerDeletionRange(selection, pastes, direction);
    if (!deletion) return false;
    currentView.dispatch({
      changes: deletion,
      selection: EditorSelection.cursor(deletion.from),
      annotations: isolateHistory.of("full"),
      userEvent: "delete",
    });
    return true;
  }

  $effect(() => {
    composerHistory = loadComposerHistory(localStorage, historyScope);
  });

  onMount(() => {
    view = new EditorView({
      parent: mountElement,
      state: EditorState.create({
        extensions: [
          history(),
          Prec.highest(EditorView.domEventHandlers({ keydown: completionKeydown })),
          Prec.highest(keymap.of([
            {
              key: "Mod-z",
              run: (currentView) => currentView.state.readOnly,
            },
            {
              key: "Mod-Shift-z",
              run: (currentView) => currentView.state.readOnly,
            },
            {
              key: "Mod-y",
              run: (currentView) => currentView.state.readOnly,
            },
            {
              key: "ArrowUp",
              run: (currentView) => browseHistory(currentView, "older"),
            },
            {
              key: "ArrowDown",
              run: (currentView) => browseHistory(currentView, "newer"),
            },
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
          editable.of([
            EditorView.editable.of(!disabled),
            EditorState.readOnly.of(disabled),
          ]),
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
            if (update.docChanged || update.selectionSet) queueMicrotask(() => refreshCompletion());
            if (update.docChanged && !restoringHistory) composerHistory.cancelNavigation();
            if (update.docChanged || update.transactions.some((tx) => tx.effects.length > 0)) {
              emitChange();
            }
          }),
          Prec.high(EditorView.domEventHandlers({
            focus() { queueMicrotask(() => refreshCompletion(true)); return false; },
            blur() { closeCompletion(); return false; },
            compositionend() { queueMicrotask(() => refreshCompletion(true)); return false; },
            paste(event) {
              return handlePasteEvent(event);
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
      closeCompletion();
      view?.destroy();
      view = null;
    };
  });

  $effect(() => {
    const isDisabled = disabled;
    view?.dispatch({
      effects: editable.reconfigure([
        EditorView.editable.of(!isDisabled),
        EditorState.readOnly.of(isDisabled),
      ]),
    });
  });

  export function snapshot(): ComposerDraftSnapshot {
    return currentSnapshot();
  }

  export function recordHistory(value: ComposerDraftSnapshot): void {
    const entry: ComposerHistoryEntry = {
      segments: value.segments,
      preserveExactText: value.textPastes.length > 0,
    };
    if (composerHistory.record(entry)) {
      saveComposerHistory(localStorage, historyScope, composerHistory);
    }
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
    if (!view || view.state.readOnly) return;
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

  export function restoreSegments(
    segments: readonly Segment[],
    preserveExactText = false,
  ): void {
    if (!view) return;
    let document = "";
    const pasteEffects: StateEffect<{ key: number; paste: ComposerPaste }>[] = [];
    const textEffects: StateEffect<ComposerTextPaste>[] = [];
    let highestPasteId = nextPasteId - 1;
    for (const segment of segments) {
      if (segment.kind === "text") {
        const rendered = view.state.toText(segment.content).toString();
        const from = document.length;
        document += rendered;
        if (preserveExactText) {
          textEffects.push(registerTextPaste.of({
            from,
            to: from + rendered.length,
            rendered,
            content: segment.content,
          }));
        }
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
        pasteEffects.push(registerPaste.of({ key, paste }));
      } else if (segment.kind === "file_ref") {
        document += `@${segment.path}`;
      }
    }
    nextPasteId = highestPasteId + 1;
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: document },
      selection: EditorSelection.cursor(document.length),
      effects: [...pasteEffects, ...textEffects],
      userEvent: "input.restore",
    });
  }
</script>

<div class="composer-input" class:disabled>
  {#if completionToken}
    <div class="completion-popup">
      <div id={completionId} role="listbox" aria-label="Composer completions" aria-busy={completionBusy}>
        {#each visibleCompletions as entry, offset (entry.value)}
          {@const index = completionStart + offset}
          <button
            id={completionId + "-" + index}
            type="button" role="option" tabindex="-1"
            aria-selected={completionSelected === index}
            title={entry.usage ?? entry.value}
            onpointerdown={(event) => event.preventDefault()}
            onclick={() => acceptCompletion("accept", index)}
          >
            <span class="completion-value">{completionToken.sigil}{entry.value}{entry.is_dir && !entry.value.endsWith("/") ? "/" : ""}</span>
            {#if entry.description}<span class="completion-description">{entry.description}</span>{/if}
          </button>
        {/each}
      </div>
      <div class="completion-hint" role="status">
        {#if completionBusy}Loading files…{:else if completionMessage}{completionMessage}{:else}↑↓ select · Tab complete · Enter confirm · Esc close{/if}
      </div>
    </div>
  {/if}
  <div bind:this={mountElement}></div>
</div>

<style>
  .composer-input {
    position: relative;
    min-width: 0;
    flex: 1;
    color: var(--text-strong);
    font: inherit;
  }

  .completion-popup {
    position: absolute;
    bottom: 100%;
    left: 0;
    right: 0;
    z-index: 10;
    padding: var(--space-1);
    background: var(--bg-raised);
    box-shadow: var(--shadow-overlay);
    color: var(--text);
    font-size: var(--font-size-compact);
    line-height: var(--line-height-compact);
  }

  .completion-popup button {
    display: flex;
    gap: var(--space-3);
    width: 100%;
    min-width: 0;
    padding: var(--space-1) var(--space-2);
    border: 0;
    border-radius: 0;
    background: transparent;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }

  .completion-popup button:hover { background: var(--interactive-hover); }
  .completion-popup button[aria-selected="true"] { background: var(--interactive-selected); }
  .completion-value { flex: 0 1 auto; font-family: var(--font-mono); }
  .completion-description { flex: 1 1 auto; color: var(--text-muted); }
  .completion-value, .completion-description {
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .completion-hint { padding: var(--space-1) var(--space-2); color: var(--text-muted); }

  .composer-input.disabled {
    opacity: 0.56;
  }

  .composer-input :global(.cm-editor) {
    min-height: 2.65rem;
    max-height: 10rem;
  }

  .composer-input :global(.cm-scroller) {
    overflow-y: auto;
  }

  .composer-input :global(.cm-content) {
    min-height: 2.65rem;
    padding: var(--space-2) var(--space-2) var(--space-1);
    line-height: var(--line-height-body);
  }

  .composer-input :global(.cm-line) {
    padding: 0;
  }

  .composer-input :global(.composer-paste-chip) {
    display: inline-flex;
    align-items: center;
    max-width: min(26rem, 70vw);
    margin: 0 var(--space-1);
    padding: 0 var(--space-2);
    overflow: hidden;
    border: 1px solid color-mix(in srgb, var(--accent) 42%, var(--line));
    border-radius: 999px;
    background: color-mix(in srgb, var(--accent) 10%, var(--bg-subtle));
    color: var(--text-muted);
    font-size: var(--font-size-compact);
    font-weight: 600;
    line-height: var(--line-height-compact);
    text-overflow: ellipsis;
    vertical-align: baseline;
    white-space: nowrap;
  }
</style>
