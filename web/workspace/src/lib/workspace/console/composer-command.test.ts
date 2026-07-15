import {
  buildComposerRequest,
  parseSigilSegments,
} from "./composer-command.ts";

declare const Deno: {
  test(name: string, fn: () => void): void;
};

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) {
    throw new Error(message);
  }
}

function assertEquals<T>(actual: T, expected: T): void {
  const actualJson = JSON.stringify(actual);
  const expectedJson = JSON.stringify(expected);
  if (actualJson !== expectedJson) {
    throw new Error(`Expected ${expectedJson}, got ${actualJson}`);
  }
}

Deno.test("parseSigilSegments converts TUI-style references", () => {
  assertEquals(
    parseSigilSegments("read @src/main.rs then #memory and /literal"),
    [
      { kind: "text", content: "read " },
      { kind: "file_ref", path: "src/main.rs" },
      { kind: "text", content: " then " },
      { kind: "knowledge_ref", slug: "memory" },
      { kind: "text", content: " and /literal" },
    ],
  );
});

Deno.test("buildComposerRequest sends user segments when sigils are present", () => {
  const result = buildComposerRequest("inspect @README.md");
  assert(result.ok, "request should be accepted");
  assertEquals(result.request?.kind, "user");
  assertEquals(result.request?.content, "inspect @README.md");
  assertEquals(result.request?.segments, [
    { kind: "text", content: "inspect " },
    { kind: "file_ref", path: "README.md" },
  ]);
});

Deno.test("buildComposerRequest parses colon commands", () => {
  const compact = buildComposerRequest(":compact");
  assert(compact.ok, "compact should be accepted");
  assertEquals(compact.request, { kind: "compact", content: "" });

  const peer = buildComposerRequest(":peer companion");
  assert(peer.ok, "peer should be accepted");
  assertEquals(peer.request, { kind: "register_peer", content: "companion" });

  const help = buildComposerRequest(":help compact");
  assert(help.ok, "help should be accepted");
  assertEquals(help.request, undefined);
  assert(
    help.notice?.includes(":compact"),
    "help should return a local notice",
  );
});

Deno.test("buildComposerRequest rejects invalid colon commands", () => {
  const unknown = buildComposerRequest(":does-not-exist");
  assert(!unknown.ok, "unknown command should be rejected");

  const invalidPeer = buildComposerRequest(":peer");
  assert(!invalidPeer.ok, "invalid peer command should be rejected");
});
