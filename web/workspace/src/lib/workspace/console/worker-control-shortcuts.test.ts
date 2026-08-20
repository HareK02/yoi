// @ts-nocheck
import { resolveWorkerControlShortcut } from "./worker-control-shortcuts.ts";

function assertEquals(actual: unknown, expected: unknown): void {
  if (actual !== expected) {
    throw new Error(`expected ${String(expected)}, got ${String(actual)}`);
  }
}

const base = {
  protocolOpen: true,
  running: false,
  paused: false,
  composerFocused: false,
  draftBlank: true,
  editableTarget: false,
  hasSelection: false,
};

Deno.test("Worker control shortcuts match TUI pause cancel and resume keys", () => {
  assertEquals(
    resolveWorkerControlShortcut(
      { key: "c", ctrlKey: true },
      { ...base, running: true },
    ),
    "pause",
  );
  assertEquals(
    resolveWorkerControlShortcut(
      { key: "x", ctrlKey: true },
      { ...base, paused: true },
    ),
    "cancel",
  );
  assertEquals(
    resolveWorkerControlShortcut(
      { key: "Enter" },
      { ...base, paused: true, composerFocused: true },
    ),
    "resume",
  );
});

Deno.test("Worker control shortcuts preserve browser editing operations", () => {
  for (
    const state of [
      { ...base, running: true, editableTarget: true },
      { ...base, running: true, hasSelection: true },
    ]
  ) {
    assertEquals(
      resolveWorkerControlShortcut({ key: "c", ctrlKey: true }, state),
      null,
    );
  }
  assertEquals(
    resolveWorkerControlShortcut(
      { key: "x", ctrlKey: true },
      { ...base, running: true, editableTarget: true },
    ),
    null,
  );
});

Deno.test("Resume requires a blank focused composer and paused Worker", () => {
  assertEquals(
    resolveWorkerControlShortcut(
      { key: "Enter" },
      { ...base, paused: true, composerFocused: true, draftBlank: false },
    ),
    null,
  );
  assertEquals(
    resolveWorkerControlShortcut(
      { key: "Enter" },
      { ...base, paused: true, composerFocused: false },
    ),
    null,
  );
});
