import type { Segment } from "../src/lib/generated/protocol.ts";
import {
  COMPOSER_HISTORY_LIMIT,
  ComposerHistory,
  type ComposerHistoryEntry,
  composerHistoryStorageKey,
  loadComposerHistory,
  saveComposerHistory,
  shouldBrowseComposerHistory,
} from "../src/lib/workspace/console/composer-history.ts";

function assert(
  condition: unknown,
  message = "assertion failed",
): asserts condition {
  if (!condition) throw new Error(message);
}

function assertEquals(actual: unknown, expected: unknown): void {
  const actualJson = JSON.stringify(actual);
  const expectedJson = JSON.stringify(expected);
  if (actualJson !== expectedJson) {
    throw new Error(`expected ${expectedJson}, received ${actualJson}`);
  }
}

function entry(content: string): ComposerHistoryEntry {
  return {
    segments: [{ kind: "text", content }],
    preserveExactText: false,
  };
}

function text(entryValue: ComposerHistoryEntry | null): string | null {
  const segment = entryValue?.segments[0];
  return segment?.kind === "text" ? segment.content : null;
}

function memoryStorage(initial: Record<string, string> = {}) {
  const values = new Map(Object.entries(initial));
  return {
    getItem(key: string): string | null {
      return values.get(key) ?? null;
    },
    setItem(key: string, value: string): void {
      values.set(key, value);
    },
    value(key: string): string | null {
      return values.get(key) ?? null;
    },
  };
}

Deno.test("Composer history skips blank and consecutive duplicate entries", () => {
  const history = new ComposerHistory();

  assert(!history.record(entry("  \n")));
  assert(history.record(entry("first")));
  assert(!history.record(entry("first")));
  assert(history.record(entry("second")));
  assertEquals(history.entries.map(text), ["first", "second"]);
});

Deno.test("Composer history keeps the newest 30 entries", () => {
  const history = new ComposerHistory();
  for (let index = 0; index < COMPOSER_HISTORY_LIMIT + 4; index += 1) {
    history.record(entry(`message-${index}`));
  }

  assertEquals(history.entries.length, COMPOSER_HISTORY_LIMIT);
  assertEquals(text(history.entries[0] ?? null), "message-4");
  assertEquals(text(history.entries.at(-1) ?? null), "message-33");
});

Deno.test("Composer history uses only the multiline input boundaries", () => {
  const base = {
    lineCount: 3,
    selectionEmpty: true,
    readOnly: false,
    composing: false,
  };

  assert(
    shouldBrowseComposerHistory({ ...base, direction: "older", cursorLine: 1 }),
  );
  assert(
    !shouldBrowseComposerHistory({
      ...base,
      direction: "older",
      cursorLine: 2,
    }),
  );
  assert(
    shouldBrowseComposerHistory({ ...base, direction: "newer", cursorLine: 3 }),
  );
  assert(
    !shouldBrowseComposerHistory({
      ...base,
      direction: "newer",
      cursorLine: 2,
    }),
  );
  assert(
    !shouldBrowseComposerHistory({
      ...base,
      direction: "older",
      cursorLine: 1,
      selectionEmpty: false,
    }),
  );
  assert(
    !shouldBrowseComposerHistory({
      ...base,
      direction: "newer",
      cursorLine: 3,
      readOnly: true,
    }),
  );
  assert(
    !shouldBrowseComposerHistory({
      ...base,
      direction: "older",
      cursorLine: 1,
      composing: true,
    }),
  );
});

Deno.test("Composer history navigates older and restores the draft after newer", () => {
  const history = new ComposerHistory([entry("first"), entry("second")]);

  assertEquals(text(history.previous(entry("unsent draft"))), "second");
  assertEquals(text(history.previous(entry("ignored draft"))), "first");
  assertEquals(text(history.previous(entry("ignored draft"))), "first");
  assertEquals(text(history.next()), "second");
  assertEquals(text(history.next()), "unsent draft");
  assert(!history.browsing);
  assertEquals(history.next(), null);
});

Deno.test("editing cancels Composer history navigation", () => {
  const history = new ComposerHistory([entry("sent")]);
  history.previous(entry("draft"));
  assert(history.browsing);

  history.cancelNavigation();

  assert(!history.browsing);
  assertEquals(history.next(), null);
});

Deno.test("Composer history persists segments by workspace and ignores corrupt storage", () => {
  const storage = memoryStorage();
  const workspaceId = "workspace / one";
  const history = new ComposerHistory();
  const paste = {
    kind: "paste",
    id: 7,
    content: "large paste",
    chars: 11,
    lines: 1,
  } satisfies Segment;
  history.record({ segments: [paste], preserveExactText: true });

  saveComposerHistory(storage, workspaceId, history);
  const restored = loadComposerHistory(storage, workspaceId);

  assertEquals(restored.entries, history.entries);
  assertEquals(
    composerHistoryStorageKey(workspaceId),
    "yoi.composer-history.v1.workspace.workspace%20%2F%20one",
  );

  const corrupt = memoryStorage({
    [composerHistoryStorageKey(workspaceId)]: "not-json",
  });
  assertEquals(loadComposerHistory(corrupt, workspaceId).entries, []);
});

Deno.test("Composer input uses boundary-aware Up and Down history navigation", async () => {
  const inputSource = await Deno.readTextFile(
    new URL(
      "../src/lib/workspace/console/ComposerInput.svelte",
      import.meta.url,
    ),
  );
  const consoleSource = await Deno.readTextFile(
    new URL(
      "../src/routes/w/[workspaceId]/runtimes/[runtimeId]/workers/[workerId]/console/+page.svelte",
      import.meta.url,
    ),
  );

  assert(inputSource.includes('key: "ArrowUp"'));
  assert(inputSource.includes('key: "ArrowDown"'));
  assert(inputSource.includes("shouldBrowseComposerHistory({"));
  assert(inputSource.includes("lineCount: currentView.state.doc.lines"));
  assert(inputSource.includes("composerHistory.cancelNavigation()"));
  assert(consoleSource.includes("historyScope={workspaceId}"));
  assert(consoleSource.includes("composerInputElement?.recordHistory(value)"));
});
