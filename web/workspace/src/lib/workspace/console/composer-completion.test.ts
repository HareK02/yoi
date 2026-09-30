import {
  applyCompletion,
  completionSelection,
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

Deno.test("completionTokenAt detects command and file sigils before the cursor", () => {
  assertEquals(completionTokenAt("open @src/ma", "open @src/ma".length), {
    sigil: "@",
    kind: "file",
    start: 5,
    end: 12,
    prefix: "src/ma",
  });
  assertEquals(completionTokenAt(":comp", 5)?.kind, "command");
  assertEquals(completionTokenAt("run /work", 9), null);
  assertEquals(completionTokenAt("ask #plain", "ask #plain".length), null);
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

Deno.test("completion scope, aliases and middle-of-token replacement match command semantics", () => {
  assertEquals(completionTokenAt("explain :comp", 13), null);
  assertEquals(completionTokenAt(":notify @file", 13), null);
  assertEquals(completionTokenAt("@\uFFF91\uFFFB", 4), null);
  assertEquals(localCommandCompletions("roll").map((entry) => entry.value), [
    "rewind",
  ]);
  assertEquals(localCommandCompletions("?").map((entry) => entry.value), [
    "help",
  ]);
  const value = ":notify old argument";
  const token = completionTokenAt(value, 4)!;
  assertEquals(
    applyCompletion(value, token, { value: "peer" }).value,
    ":peer old argument",
  );
  const file = "see @sr trailing";
  assertEquals(
    applyCompletion(file, completionTokenAt(file, 7)!, {
      value: "src/",
      is_dir: true,
    }).value,
    "see @src/ trailing",
  );
  assertEquals(
    applyCompletion(
      "@ma",
      completionTokenAt("@ma", 3)!,
      { value: "main.rs" },
      "tab",
    ).value,
    "@main.rs",
  );
  assertEquals(completionSelection(null, 6, -1), 5);
  assertEquals(completionSelection(5, 6, 1), 0);
});

Deno.test("localCommandCompletions filters colon commands", () => {
  assertEquals(localCommandCompletions("com").map((entry) => entry.value), [
    "compact",
  ]);
});
