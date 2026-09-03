import type { Segment } from "$lib/generated/protocol";

export const COMPOSER_HISTORY_LIMIT = 30;
const COMPOSER_HISTORY_VERSION = 1;
const COMPOSER_HISTORY_KEY_PREFIX = "yoi.composer-history.v1.workspace.";

export type ComposerHistoryDirection = "older" | "newer";

export type ComposerHistoryCursor = {
  direction: ComposerHistoryDirection;
  cursorLine: number;
  lineCount: number;
  selectionEmpty: boolean;
  readOnly: boolean;
  composing: boolean;
};

export type ComposerHistoryEntry = {
  segments: Segment[];
  preserveExactText: boolean;
};

type StoredComposerHistory = {
  version: typeof COMPOSER_HISTORY_VERSION;
  entries: ComposerHistoryEntry[];
};

type ComposerHistoryStorage = Pick<Storage, "getItem" | "setItem">;

function cloneEntry(entry: ComposerHistoryEntry): ComposerHistoryEntry {
  return {
    segments: entry.segments.map((segment) => ({ ...segment })) as Segment[],
    preserveExactText: entry.preserveExactText,
  };
}

function isSegment(value: unknown): value is Segment {
  if (!value || typeof value !== "object") return false;
  const segment = value as Record<string, unknown>;
  if (segment.kind === "text") return typeof segment.content === "string";
  if (segment.kind === "paste") {
    return typeof segment.content === "string" &&
      typeof segment.id === "number" &&
      typeof segment.chars === "number" &&
      typeof segment.lines === "number";
  }
  if (segment.kind === "file_ref") return typeof segment.path === "string";
  return false;
}

function isHistoryEntry(value: unknown): value is ComposerHistoryEntry {
  if (!value || typeof value !== "object") return false;
  const entry = value as Record<string, unknown>;
  return Array.isArray(entry.segments) &&
    entry.segments.every(isSegment) &&
    typeof entry.preserveExactText === "boolean";
}

function isBlankEntry(entry: ComposerHistoryEntry): boolean {
  return entry.segments.length === 0 ||
    entry.segments.every((segment) =>
      segment.kind === "text" && segment.content.trim().length === 0
    );
}

function sameEntry(
  left: ComposerHistoryEntry,
  right: ComposerHistoryEntry,
): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

export function shouldBrowseComposerHistory(
  cursor: ComposerHistoryCursor,
): boolean {
  if (cursor.readOnly || cursor.composing || !cursor.selectionEmpty) {
    return false;
  }
  return cursor.direction === "older"
    ? cursor.cursorLine === 1
    : cursor.cursorLine === cursor.lineCount;
}

export function composerHistoryStorageKey(workspaceId: string): string {
  return `${COMPOSER_HISTORY_KEY_PREFIX}${encodeURIComponent(workspaceId)}`;
}

export class ComposerHistory {
  #entries: ComposerHistoryEntry[];
  #index: number | null = null;
  #draft: ComposerHistoryEntry | null = null;

  constructor(entries: ComposerHistoryEntry[] = []) {
    this.#entries = [];
    for (const entry of entries) this.record(entry);
  }

  get entries(): ComposerHistoryEntry[] {
    return this.#entries.map(cloneEntry);
  }

  get browsing(): boolean {
    return this.#index !== null;
  }

  record(entry: ComposerHistoryEntry): boolean {
    if (isBlankEntry(entry)) {
      this.cancelNavigation();
      return false;
    }
    const last = this.#entries.at(-1);
    if (last && sameEntry(last, entry)) {
      this.cancelNavigation();
      return false;
    }
    this.#entries.push(cloneEntry(entry));
    if (this.#entries.length > COMPOSER_HISTORY_LIMIT) {
      this.#entries.splice(0, this.#entries.length - COMPOSER_HISTORY_LIMIT);
    }
    this.cancelNavigation();
    return true;
  }

  previous(draft: ComposerHistoryEntry): ComposerHistoryEntry | null {
    if (this.#entries.length === 0) return null;
    if (this.#index === null) {
      this.#draft = cloneEntry(draft);
      this.#index = this.#entries.length - 1;
    } else if (this.#index > 0) {
      this.#index -= 1;
    }
    return cloneEntry(this.#entries[this.#index]);
  }

  next(): ComposerHistoryEntry | null {
    if (this.#index === null) return null;
    if (this.#index < this.#entries.length - 1) {
      this.#index += 1;
      return cloneEntry(this.#entries[this.#index]);
    }
    const draft = this.#draft
      ? cloneEntry(this.#draft)
      : { segments: [], preserveExactText: false };
    this.cancelNavigation();
    return draft;
  }

  cancelNavigation(): void {
    this.#index = null;
    this.#draft = null;
  }
}

export function loadComposerHistory(
  storage: ComposerHistoryStorage,
  workspaceId: string,
): ComposerHistory {
  try {
    const raw = storage.getItem(composerHistoryStorageKey(workspaceId));
    if (!raw) return new ComposerHistory();
    const value = JSON.parse(raw) as unknown;
    if (!value || typeof value !== "object") return new ComposerHistory();
    const stored = value as Partial<StoredComposerHistory>;
    if (
      stored.version !== COMPOSER_HISTORY_VERSION ||
      !Array.isArray(stored.entries)
    ) {
      return new ComposerHistory();
    }
    return new ComposerHistory(stored.entries.filter(isHistoryEntry));
  } catch {
    return new ComposerHistory();
  }
}

export function saveComposerHistory(
  storage: ComposerHistoryStorage,
  workspaceId: string,
  history: ComposerHistory,
): void {
  const value: StoredComposerHistory = {
    version: COMPOSER_HISTORY_VERSION,
    entries: history.entries,
  };
  try {
    storage.setItem(
      composerHistoryStorageKey(workspaceId),
      JSON.stringify(value),
    );
  } catch {
    // History is an optional convenience; storage failures must not block input submission.
  }
}
