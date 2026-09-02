import {
  buildComposerRequest,
  buildComposerSegmentsRequest,
  parseSigilSegments,
} from "./composer-command.ts";

declare const Deno: { test(name: string, fn: () => void): void };

function assertEquals<T>(actual: T, expected: T): void {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a !== e) throw new Error(`Expected ${e}, got ${a}`);
}

Deno.test("parseSigilSegments turns file sigils into file refs", () => {
  assertEquals(parseSigilSegments("read @src/main.rs"), [
    { kind: "text", content: "read " },
    { kind: "file_ref", path: "src/main.rs" },
  ]);
});

Deno.test("parseSigilSegments leaves hash sigils as plain text", () => {
  assertEquals(parseSigilSegments("ask #memory"), [{
    kind: "text",
    content: "ask #memory",
  }]);
});

Deno.test("uploaded-file-only input remains a typed run request", () => {
  const file = {
    artifact_id: "01900000-0000-7000-8000-000000000001",
    file_name: "notes.md",
    media_type: "text/markdown",
    created_at_ms: 1,
    availability: "available" as const,
    byte_len: 12,
    sha256: "a".repeat(64),
  };
  assertEquals(buildComposerSegmentsRequest([{ kind: "uploaded_file", file }]), {
    ok: true,
    request: {
      kind: "user",
      content: "[Attached file: notes.md]",
      segments: [{ kind: "uploaded_file", file }],
    },
  });
});

Deno.test("notify command exposes the operation instead of a System-role input", () => {
  assertEquals(buildComposerRequest(":notify reread the Ticket"), {
    ok: true,
    request: { kind: "notify", content: "reread the Ticket" },
  });
  assertEquals(buildComposerRequest(":system reread the Ticket"), {
    ok: false,
    message: "Unknown command: system. Type :help for available commands.",
  });
});
