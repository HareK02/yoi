import type { Segment } from "$lib/generated/protocol.ts";

const PASTE_TOKEN_PREFIX = "\uFFF9";
const PASTE_TOKEN_SUFFIX = "\uFFFB";
const PASTE_TOKEN_PATTERN = /\uFFF9(\d+)\uFFFB/g;

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

export interface ComposerDraftSnapshot {
  document: string;
  content: string;
  segments: Segment[];
  pastes: ComposerPasteAtom[];
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
): ComposerDraftSnapshot {
  const pastes = composerPasteAtoms(document, registry);
  const segments: Segment[] = [];
  let content = "";
  let cursor = 0;

  for (const paste of pastes) {
    const text = document.slice(cursor, paste.from);
    appendTextSegment(segments, text);
    content += text;
    segments.push({
      kind: "paste",
      id: paste.id,
      content: paste.content,
      chars: paste.chars,
      lines: paste.lines,
    });
    content += paste.content;
    cursor = paste.to;
  }

  const trailingText = document.slice(cursor);
  appendTextSegment(segments, trailingText);
  content += trailingText;

  return { document, content, segments, pastes };
}

export function pasteChipLabel(paste: ComposerPaste): string {
  const chars = paste.chars === 1 ? "char" : "chars";
  const lines = paste.lines === 1 ? "line" : "lines";
  return `Clipboard #${paste.id} · ${paste.chars} ${chars} · ${paste.lines} ${lines}`;
}
