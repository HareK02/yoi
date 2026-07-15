import {
  applyCompletion,
  completionTokenAt,
  localCommandCompletions,
} from "./composer-completion.ts";

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

Deno.test("completionTokenAt detects TUI-style sigils before the cursor", () => {
  assertEquals(completionTokenAt("open @src/ma", "open @src/ma".length), {
    sigil: "@",
    kind: "file",
    start: 5,
    end: 12,
    prefix: "src/ma",
  });
  assertEquals(completionTokenAt(":comp", 5)?.kind, "command");
  assertEquals(completionTokenAt("run /work", 9), null);
});

Deno.test("applyCompletion replaces the active token and advances the cursor", () => {
  const value = "open @src/ma please";
  const token = completionTokenAt(value, "open @src/ma".length);
  assert(token, "token should exist");
  assertEquals(applyCompletion(value, token, { value: "src/main.rs" }), {
    value: "open @src/main.rs please",
    cursor: "open @src/main.rs ".length,
  });
  assertEquals(applyCompletion(value, token, { value: "src", is_dir: true }), {
    value: "open @src/ please",
    cursor: "open @src/".length,
  });
});

Deno.test("localCommandCompletions filters colon commands", () => {
  assertEquals(localCommandCompletions("com").map((entry) => entry.value), [
    "compact",
  ]);
});
