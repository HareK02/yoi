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

Deno.test("completionTokenAt detects command, file, and Feature sigils before the cursor", () => {
  assertEquals(completionTokenAt("open @src/ma", "open @src/ma".length), {
    sigil: "@",
    kind: "file",
    start: 5,
    end: 12,
    prefix: "src/ma",
  });
  assertEquals(completionTokenAt(":comp", 5)?.kind, "command");
  assertEquals(completionTokenAt("run /work", 9), {
    sigil: "/",
    kind: "feature",
    start: 4,
    end: 9,
    prefix: "work",
  });
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

Deno.test("argument completions replace whole quoted values and preserve following arguments", () => {
  const descriptor = { identity: "test/run", name: "run", aliases: [], display_name: "Run", description: "Test",
    syntax: "parenthesized" as const, arguments: [{ name: "path", position: 0, required: true,
      value_type: { kind: "worker_file" as const }, completion: { kind: "worker_file" as const } }] };
  for (const value of ['/run("src/ma', '/run("src/ma", next=true)', '/run(path="src/ma", next=true)']) {
    const cursor = value.indexOf("ma") + 1;
    const token = completionTokenAt(value, cursor, [descriptor]);
    assert(token, "argument token should exist");
    assertEquals(token.prefix, "src/m");
    assertEquals(applyCompletion(value, token, { value: '資料/a "b"/c' }).value,
      value.startsWith('/run(path') ? '/run(path="資料/a \\"b\\"/c", next=true)' :
      value.endsWith('true)') ? '/run("資料/a \\"b\\"/c", next=true)' : '/run("資料/a \\"b\\"/c"a');
  }
});

Deno.test("incomplete or invalid quoted completion preserves mode and body after the cursor", () => {
  const descriptor = { identity: "test/run", name: "run", aliases: [], display_name: "Run", description: "Test",
    syntax: "parenthesized" as const, arguments: [{ name: "path", position: 0, required: true,
      value_type: { kind: "worker_file" as const }, completion: { kind: "worker_file" as const } }] };
  for (const value of ['/run(path="ab, mode=safe) following prose', '/run("ab, mode=safe) following prose', String.raw`/run(path="ab\q", mode=safe) following prose`]) {
    const cursor = value.indexOf("ab") + 2;
    const token = completionTokenAt(value, cursor, [descriptor], [{ from: 0, descriptor }]);
    assert(token, "selected argument completion should exist");
    assertEquals(token.end, cursor);
    const result = applyCompletion(value, token, { value: "completed/資料" });
    assertEquals(result.value.slice(result.cursor), value.slice(cursor));
    assert(result.value.includes("mode=safe) following prose"), "other arguments and body must remain intact");
  }
});

Deno.test("quoted call-looking data completes the selected outer value without erasing other arguments", () => {
  const descriptor = { identity: "test/run", name: "run", aliases: [], display_name: "Run", description: "Test",
    syntax: "parenthesized" as const, arguments: [{ name: "path", position: 0, required: true,
      value_type: { kind: "worker_file" as const }, completion: { kind: "worker_file" as const } }] };
  const value = '/run("a /run(pa", mode=safe) prose';
  const token = completionTokenAt(value, value.lastIndexOf("pa") + 2, [descriptor], [{ from: 0, descriptor }]);
  assert(token, "outer argument completion should exist");
  assertEquals(token.context?.argument, "path");
  assertEquals(token.prefix, "a /run(pa");
  assertEquals(applyCompletion(value, token, { value: "replacement" }).value, '/run("replacement", mode=safe) prose');
});

Deno.test("argument name replacement preserves its value and directories drill inside quotes", () => {
  const descriptor = { identity: "test/run", name: "run", aliases: [], display_name: "Run", description: "Test",
    syntax: "parenthesized" as const, arguments: [{ name: "path", position: 0, required: true,
      value_type: { kind: "worker_file" as const }, completion: { kind: "worker_file" as const } }] };
  const named = '/run(path="original")';
  const token = completionTokenAt(named, 7, [descriptor]);
  assert(token, "named completion should exist");
  assertEquals(applyCompletion(named, token, { value: "path=" }).value, named);
  const directory = '/run("sr")';
  const result = applyCompletion(directory, completionTokenAt(directory, 8, [descriptor])!, { value: "src", is_dir: true });
  assertEquals(result.value, '/run("src/")');
  assertEquals(completionTokenAt(result.value, result.cursor, [descriptor])?.prefix, "src/");
});
