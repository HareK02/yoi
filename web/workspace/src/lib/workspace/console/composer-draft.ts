import type { Segment } from "#lib/generated/protocol.ts";
import {
  composerTypedAtoms as editingTypedAtoms,
  snapshotComposerDraft as snapshotEditingDraft,
  type ComposerDraftSnapshot as EditingDraftSnapshot,
  type ComposerPaste,
  type ComposerTextPaste,
  type ComposerTypedAtom as EditingTypedAtom,
  type ComposerTypedSegment as EditingTypedSegment,
} from "#lib/workspace/composer/composer-draft.ts";

export {
  composerDeletionRange,
  composerPasteAtoms,
  composerPasteToken,
  composerTypedToken,
  pasteChipLabel,
  type ComposerPaste,
  type ComposerPasteAtom,
  type ComposerSelection,
  type ComposerTextPaste,
} from "#lib/workspace/composer/composer-draft.ts";

// Workerのpayloadは生成型のまま保持し、共通編集部品にprotocol enumを複製しない。
export type ComposerDraftSnapshot = EditingDraftSnapshot<Segment>;
export type ComposerTypedSegment = EditingTypedSegment<Segment>;
export type ComposerTypedAtom = EditingTypedAtom<Segment>;
export const composerTypedAtoms = editingTypedAtoms<Segment>;

export function snapshotComposerDraft(
  document: string,
  registry: ReadonlyMap<number, ComposerPaste>,
  candidateTextPastes: readonly ComposerTextPaste[] = [],
  typedRegistry: ReadonlyMap<number, ComposerTypedSegment> = new Map(),
): ComposerDraftSnapshot {
  const snapshot = snapshotEditingDraft(document, registry, candidateTextPastes, typedRegistry);
  const segments: Segment[] = [];
  for (const part of snapshot.segments) {
    const segment = part.kind === "atom" ? part.value : part;
    const previous = segments.at(-1);
    if (part.kind === "text" && previous?.kind === "text") {
      previous.content += part.content;
    } else {
      segments.push(segment.kind === "text" ? { ...segment } : segment);
    }
  }
  return { ...snapshot, segments };
}
