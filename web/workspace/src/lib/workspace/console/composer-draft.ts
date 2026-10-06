import type { Segment } from "#lib/generated/protocol.ts";

const PASTE_TOKEN_PREFIX = "\uFFF9";
const PASTE_TOKEN_SUFFIX = "\uFFFB";
const PASTE_TOKEN_PATTERN = /\uFFF9(\d+)\uFFFB/g;
const TYPED_TOKEN_PREFIX = "\uFFF8";
const TYPED_TOKEN_SUFFIX = "\uFFFA";
const TYPED_TOKEN_PATTERN = /\uFFF8(\d+)\uFFFA/g;

export interface ComposerPaste {
  id: number;
  content: string;
  chars: number;
  lines: number;
}

export interface ComposerPasteAtom extends ComposerPaste {
  key: number;
  from: number;
  to: number;
}

export interface ComposerTextPaste {
  from: number;
  to: number;
  rendered: string;
  content: string;
}

export interface ComposerTypedSegment {
  segment: Segment;
  label: string;
  /** Whether deleting this unsent atom must release client-staged state. */
  cleanup?: boolean;
  /** Client-only reservation whose removal cancels an in-progress upload. */
  uploadReservation?: boolean;
}

export interface ComposerTypedAtom extends ComposerTypedSegment {
  key: number;
  from: number;
  to: number;
}

export interface ComposerSelection {
  from: number;
  to: number;
  head: number;
}

export function composerDeletionRange(
  selection: ComposerSelection,
  pastes: readonly { from: number; to: number }[],
  direction: "backward" | "forward",
): { from: number; to: number } | null {
  if (selection.from !== selection.to) {
    return { from: selection.from, to: selection.to };
  }
  const paste = direction === "backward"
    ? pastes.find((candidate) => candidate.to === selection.head)
    : pastes.find((candidate) => candidate.from === selection.head);
  return paste ? { from: paste.from, to: paste.to } : null;
}

export interface ComposerDraftSnapshot {
  document: string;
  content: string;
  segments: Segment[];
  pastes: ComposerPasteAtom[];
  textPastes: ComposerTextPaste[];
}

export function composerPasteToken(key: number): string {
  return `${PASTE_TOKEN_PREFIX}${key}${PASTE_TOKEN_SUFFIX}`;
}

export function composerPasteAtoms(
  document: string,
  registry: ReadonlyMap<number, ComposerPaste>,
): ComposerPasteAtom[] {
  const atoms: ComposerPasteAtom[] = [];
  for (const match of document.matchAll(PASTE_TOKEN_PATTERN)) {
    const key = Number(match[1]);
    const paste = registry.get(key);
    if (!paste || match.index === undefined) continue;
    atoms.push({
      ...paste,
      key,
      from: match.index,
      to: match.index + match[0].length,
    });
  }
  return atoms;
}

export function composerTypedToken(key: number): string {
  return `${TYPED_TOKEN_PREFIX}${key}${TYPED_TOKEN_SUFFIX}`;
}

export function composerTypedAtoms(
  document: string,
  registry: ReadonlyMap<number, ComposerTypedSegment>,
): ComposerTypedAtom[] {
  const atoms: ComposerTypedAtom[] = [];
  for (const match of document.matchAll(TYPED_TOKEN_PATTERN)) {
    const key = Number(match[1]);
    const atom = registry.get(key);
    if (!atom || match.index === undefined) continue;
    atoms.push({
      ...atom,
      key,
      from: match.index,
      to: match.index + match[0].length,
    });
  }
  return atoms;
}

function appendTextSegment(segments: Segment[], content: string): void {
  if (content.length === 0) return;
  const previous = segments.at(-1);
  if (previous?.kind === "text") {
    previous.content += content;
  } else {
    segments.push({ kind: "text", content });
  }
}

export function snapshotComposerDraft(
  document: string,
  registry: ReadonlyMap<number, ComposerPaste>,
  candidateTextPastes: readonly ComposerTextPaste[] = [],
  typedRegistry: ReadonlyMap<number, ComposerTypedSegment> = new Map(),
): ComposerDraftSnapshot {
  const pastes = composerPasteAtoms(document, registry);
  const typedAtoms = composerTypedAtoms(document, typedRegistry);
  const textPastes = candidateTextPastes
    .filter((paste) =>
      paste.from >= 0 &&
      paste.to <= document.length &&
      document.slice(paste.from, paste.to) === paste.rendered
    )
    .sort((left, right) => left.from - right.from);
  const events = [
    ...pastes.map((paste) => ({
      kind: "paste" as const,
      from: paste.from,
      to: paste.to,
      paste,
    })),
    ...typedAtoms.map((atom) => ({
      kind: "typed" as const,
      from: atom.from,
      to: atom.to,
      atom,
    })),
    ...textPastes.map((paste) => ({
      kind: "text_paste" as const,
      from: paste.from,
      to: paste.to,
      paste,
    })),
  ].sort((left, right) => left.from - right.from);
  const segments: Segment[] = [];
  let content = "";
  let cursor = 0;

  for (const event of events) {
    if (event.from < cursor) continue;
    const text = document.slice(cursor, event.from);
    appendTextSegment(segments, text);
    content += text;

    if (event.kind === "paste") {
      segments.push({
        kind: "paste",
        id: event.paste.id,
        content: event.paste.content,
        chars: event.paste.chars,
        lines: event.paste.lines,
      });
      content += event.paste.content;
    } else if (event.kind === "typed") {
      segments.push(event.atom.segment);
      content += event.atom.label;
    } else {
      appendTextSegment(segments, event.paste.content);
      content += event.paste.content;
    }
    cursor = event.to;
  }

  const trailingText = document.slice(cursor);
  appendTextSegment(segments, trailingText);
  content += trailingText;

  return { document, content, segments, pastes, textPastes };
}

export function pasteChipLabel(paste: ComposerPaste): string {
  const chars = paste.chars === 1 ? "char" : "chars";
  const lines = paste.lines === 1 ? "line" : "lines";
  return `Clipboard #${paste.id} · ${paste.chars} ${chars} · ${paste.lines} ${lines}`;
}
