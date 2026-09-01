import type { Segment } from "$lib/generated/protocol.ts";
import {
  type ComposerPaste,
  composerPasteToken,
  pasteChipLabel,
  snapshotComposerDraft,
} from "$lib/workspace/console/composer-draft.ts";
import { buildComposerSegmentsRequest } from "$lib/workspace/console/composer-command.ts";
import { measureComposerPaste } from "$lib/workspace/console/composer-paste.ts";

declare const Deno: {
  test(name: string, fn: () => void): void;
};

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

function paste(id: number, content: string): ComposerPaste {
  const measurement = measureComposerPaste(content);
  return {
    id,
    content,
    chars: measurement.charCount,
    lines: measurement.logicalLineCount,
  };
}

Deno.test("composer draft preserves mixed Text and Paste order exactly", () => {
  const unicodeCrlf = "🙂界\r\nsecond\r\n";
  const trailingNewline = `${"x".repeat(51)}\n`;
  const registry = new Map<number, ComposerPaste>([
    [11, paste(1, unicodeCrlf)],
    [12, paste(2, trailingNewline)],
  ]);
  const document = `before ${composerPasteToken(11)} middle ${
    composerPasteToken(12)
  } after`;

  const snapshot = snapshotComposerDraft(document, registry);

  assertEquals(
    snapshot.content,
    `before ${unicodeCrlf} middle ${trailingNewline} after`,
  );
  assertEquals(snapshot.segments, [
    { kind: "text", content: "before " },
    {
      kind: "paste",
      id: 1,
      content: unicodeCrlf,
      chars: 12,
      lines: 3,
    },
    { kind: "text", content: " middle " },
    {
      kind: "paste",
      id: 2,
      content: trailingNewline,
      chars: 52,
      lines: 2,
    },
    { kind: "text", content: " after" },
  ]);
  assertEquals(snapshot.pastes.map((entry) => entry.key), [11, 12]);
});

Deno.test("composer paste chip label is compact and accessible", () => {
  assertEquals(
    pasteChipLabel({ id: 4, content: "payload", chars: 7, lines: 1 }),
    "Clipboard #4 · 7 chars · 1 line",
  );
});

Deno.test("typed composer restoration retains Paste ids and metadata", () => {
  const original: Segment[] = [
    { kind: "text", content: "prefix\n" },
    {
      kind: "paste",
      id: 9,
      content: "alpha\r\nbeta\r\n",
      chars: 13,
      lines: 3,
    },
    { kind: "text", content: "\nsuffix" },
  ];
  const registry = new Map<number, ComposerPaste>([
    [31, original[1] as Extract<Segment, { kind: "paste" }>],
  ]);
  const restored = snapshotComposerDraft(
    `prefix\n${composerPasteToken(31)}\nsuffix`,
    registry,
  );

  assertEquals(restored.segments, original);
});

Deno.test("mixed composer request preserves Paste and parsed file-ref boundaries", () => {
  const segments: Segment[] = [
    { kind: "text", content: "inspect @src/main.rs then " },
    {
      kind: "paste",
      id: 2,
      content: "a\r\nb\r\n",
      chars: 6,
      lines: 3,
    },
    { kind: "text", content: " exactly" },
  ];

  const result = buildComposerSegmentsRequest(segments);
  assert(result.ok);
  assertEquals(result.request, {
    kind: "user",
    content: "inspect @src/main.rs then a\r\nb\r\n exactly",
    segments: [
      { kind: "text", content: "inspect " },
      { kind: "file_ref", path: "src/main.rs" },
      { kind: "text", content: " then " },
      segments[1],
      segments[2],
    ],
  });
});

Deno.test("short-paste Text preserves CRLF, trailing newline, and surrounding whitespace", () => {
  const original = "  short\r\npaste\r\n  ";
  const rendered = "  short\npaste\n  ";
  const snapshot = snapshotComposerDraft(rendered, new Map(), [{
    from: 0,
    to: rendered.length,
    rendered,
    content: original,
  }]);

  assertEquals(snapshot.content, original);
  assertEquals(snapshot.segments, [{ kind: "text", content: original }]);
  assertEquals(snapshot.textPastes.length, 1);

  const result = buildComposerSegmentsRequest(snapshot.segments, {
    preserveExactText: snapshot.textPastes.length > 0,
  });
  assert(result.ok);
  assertEquals(result.request, {
    kind: "user",
    content: original,
    segments: [{ kind: "text", content: original }],
  });
});

Deno.test("edited short-paste provenance falls back to visible Text", () => {
  const snapshot = snapshotComposerDraft("changed", new Map(), [{
    from: 0,
    to: 5,
    rendered: "short",
    content: "short\r\n",
  }]);

  assertEquals(snapshot.content, "changed");
  assertEquals(snapshot.segments, [{ kind: "text", content: "changed" }]);
  assertEquals(snapshot.textPastes, []);
});

Deno.test("Paste content beginning with a colon remains opaque user input", () => {
  const directPaste: Segment = {
    kind: "paste",
    id: 3,
    content: ":not-a-command\r\n",
    chars: 16,
    lines: 2,
  };
  const direct = buildComposerSegmentsRequest([directPaste]);
  assert(direct.ok);
  assertEquals(direct.request, {
    kind: "user",
    content: ":not-a-command\r\n",
    segments: [directPaste],
  });

  const afterWhitespace = buildComposerSegmentsRequest([
    { kind: "text", content: "  " },
    directPaste,
  ]);
  assert(afterWhitespace.ok);
  assert(afterWhitespace.request);
  assertEquals(afterWhitespace.request.kind, "user");
  assertEquals(afterWhitespace.request.content, "  :not-a-command\r\n");
});

Deno.test("plain short-paste Text retains the existing composer request path", () => {
  const result = buildComposerSegmentsRequest([
    { kind: "text", content: "  short\r\npaste\r\n  " },
  ]);
  assert(result.ok);
  assertEquals(result.request, {
    kind: "user",
    content: "short\r\npaste",
  });
});
