import { expect, test } from "vitest";
import { composerPasteToken, composerTypedToken, snapshotComposerDraft } from "./composer-draft.ts";
import { snapshotComposerDraft as workerSnapshot } from "../console/composer-draft.ts";
import type { Segment } from "#lib/generated/protocol.ts";

test("shared editing data keeps opaque adapter atoms separate from text and full paste content", () => {
  const content = "exact\r\n".repeat(100);
  const paste = { id: 1, content, chars: content.length, lines: 101 };
  const reference = { target: "independent-adapter-reference" };
  const snapshot = snapshotComposerDraft(
    `before ${composerTypedToken(1)} ${composerPasteToken(2)} after`,
    new Map([[2, paste]]),
    [],
    new Map([[1, { segment: reference, label: "Reference" }]]),
  );
  expect(snapshot.content).toBe(`before Reference ${content} after`);
  expect(snapshot.segments).toEqual([
    { kind: "text", content: "before " },
    { kind: "atom", value: reference },
    { kind: "text", content: " " },
    { kind: "paste", ...paste },
    { kind: "text", content: " after" },
  ]);
});

test("Worker adapter converts opaque editor atoms without changing typed payload or mutating text atoms", () => {
  const text: Segment = { kind: "text", content: "typed text" };
  const file: Segment = { kind: "file_ref", path: "資料/source.ts" };
  const snapshot = workerSnapshot(
    `before ${composerTypedToken(1)} after ${composerTypedToken(2)}`,
    new Map(),
    [],
    new Map([
      [1, { segment: text, label: text.content }],
      [2, { segment: file, label: `@${file.path}` }],
    ]),
  );
  expect(snapshot.segments).toEqual([
    { kind: "text", content: "before " },
    { kind: "text", content: "typed text after " },
    file,
  ]);
  expect(snapshot.segments[2]).toBe(file);
  expect(text.content).toBe("typed text");
});
