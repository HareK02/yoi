import { throws } from "node:assert/strict";
import type { FeatureInvocationDescriptor } from "#lib/generated/protocol.ts";
import {
  activeInvocationArgument,
  finalizeSelectedFeatureInvocations,
  invocationInput,
  selectedFeatureInvocationRanges,
} from "./composer-invocation.ts";

declare const Deno: { test(name: string, fn: () => void): void };

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}
function assertEquals(actual: unknown, expected: unknown): void {
  const left = JSON.stringify(actual);
  const right = JSON.stringify(expected);
  if (left !== right) throw new Error(`Expected ${right}, got ${left}`);
}

const descriptor: FeatureInvocationDescriptor = {
  identity: "builtin:test/run",
  name: "run",
  aliases: ["execute"],
  display_name: "Run",
  description: "Run a declared operation",
  syntax: "parenthesized",
  arguments: [
    {
      name: "path",
      position: 0,
      required: true,
      value_type: { kind: "worker_file" },
      completion: { kind: "worker_file" },
    },
    {
      name: "mode",
      required: false,
      value_type: { kind: "enum", values: ["safe", "fast"] },
      completion: { kind: "static", values: ["safe", "fast"] },
    },
  ],
};

Deno.test("selected invocation parsing preserves Unicode, spaces, slashes, prose, and multiple calls", () => {
  const text = '前 /run("資料/a b/c", mode=safe) 後 /execute("x/y") done';
  const ranges = selectedFeatureInvocationRanges(text, [descriptor]);
  assertEquals(ranges.length, 2);
  assertEquals(text.slice(ranges[0].end), ' 後 /execute("x/y") done');
  assertEquals(ranges[0].invocation.arguments.map((argument) => argument.value), [
    { kind: "string", value: "資料/a b/c" },
    { kind: "string", value: "safe" },
  ]);

  const segments = finalizeSelectedFeatureInvocations(
    [{ kind: "text", content: text }],
    [descriptor],
  );
  assertEquals(segments.map((segment) => segment.kind), [
    "text",
    "feature_invoke",
    "text",
    "feature_invoke",
    "text",
  ]);
});

Deno.test("unselected text, URLs, paths, and paste content are never promoted", () => {
  assertEquals(
    finalizeSelectedFeatureInvocations(
      [
        { kind: "text", content: '/run("x") https://host/run("y") /tmp/run("z")' },
        { kind: "paste", id: 1, chars: 9, lines: 1, content: '/run("p")' },
      ],
      [],
    ).map((segment) => segment.kind),
    ["text", "paste"],
  );
});

Deno.test("argument completion ignores quoted invocation-looking data and escaped quotes", () => {
  for (const [input, prefix] of [
    ['/run("a /run(pa", mode=safe) prose', 'a /run(pa'],
    [String.raw`/run(path="a \" /execute(pa", mode=safe) prose`, 'a " /execute(pa'],
    [String.raw`/run(path="a \" /run(pa, mode=safe) prose`, 'a " /run(pa'],
  ]) {
    const dataCursor = input.lastIndexOf("(pa") + 3;
    for (const occurrences of [undefined, [{ from: 0, descriptor }]]) {
      const active = activeInvocationArgument(input, dataCursor, [descriptor], occurrences);
      assertEquals(active?.descriptor.identity, descriptor.identity);
      assertEquals(active?.argument?.name, "path");
      assertEquals(active?.start, input.indexOf('"'));
      assertEquals(active?.prefix, prefix);
    }
  }
  const input = '/run("a /run(pa", mode=safe) prose /execute("new")';
  const active = activeInvocationArgument(input, input.indexOf("new") + 2, [descriptor]);
  assertEquals(active?.prefix, "ne");
  assertEquals(active?.start, input.lastIndexOf('"new'));
  assertEquals(activeInvocationArgument(input, input.indexOf("prose") + 3, [descriptor]), null);
  const unknown = '/unknown("a /run(pa", mode=safe) prose';
  assertEquals(activeInvocationArgument(unknown, unknown.lastIndexOf("pa") + 2, [descriptor]), null);
  const quotedProse = 'quoted "a /run(pa" prose';
  assertEquals(activeInvocationArgument(quotedProse, quotedProse.lastIndexOf("pa") + 2, [descriptor]), null);
});

Deno.test("editor occurrence anchors exclude unselected matching call contexts", () => {
  const input = '/run("literal /run(unfinished data before /execute("selected")';
  const from = input.indexOf("/execute(");
  const active = activeInvocationArgument(input, input.indexOf("selected") + 3, [descriptor], [{ from, descriptor }]);
  assertEquals(active?.prefix, "sel");
  assertEquals(active?.start, input.indexOf('"selected'));
  assertEquals(activeInvocationArgument(input, input.indexOf("literal") + 2, [descriptor], []), null);
});

Deno.test("incomplete quoted values bound completion replacement to the cursor", () => {
  for (const input of ['/run(path="ab, mode=safe) following prose', '/run("ab, mode=safe) following prose', String.raw`/run(path="ab\q", mode=safe) following prose`]) {
    const cursor = input.indexOf("ab") + 2;
    const active = activeInvocationArgument(input, cursor, [descriptor]);
    assertEquals(active?.end, cursor);
    assertEquals(active?.prefix, "ab");
  }
});

Deno.test("Feature strings reject lone surrogate escapes and code units like Rust JSON", () => {
  for (const literal of [String.raw`"\uD800"`, String.raw`"\uDC00"`, String.raw`"\uD800x"`, String.raw`"\uD800\uD800"`, String.raw`"\uDC00\uD800"`, JSON.stringify("\uD800"), JSON.stringify("\uDC00")]) {
    throws(() => selectedFeatureInvocationRanges(`/run(${literal})`, [descriptor]), /unpaired Unicode surrogates/);
    const input = `/run(${literal})`;
    assertEquals(activeInvocationArgument(input, input.length - 1, [descriptor]), null);
  }
  throws(() => selectedFeatureInvocationRanges('/run("\uD800")', [descriptor]), /unpaired Unicode surrogates/);
  throws(() => selectedFeatureInvocationRanges('/run(\uDC00)', [descriptor]), /unpaired Unicode surrogates/);
  for (const literal of [String.raw`"\uD83D\uDE00"`, '"😀"', String.raw`"\uDBFF\uDFFF"`, String.raw`"\u0000\b\f\n\r\t\"\\\/資料"`]) {
    const parsed = selectedFeatureInvocationRanges(`/run(${literal})`, [descriptor])[0].invocation;
    assertEquals(parsed.arguments[0].value, { kind: "string", value: JSON.parse(literal) });
  }
  for (const literal of [String.raw`"\q"`, String.raw`"\u12xz"`, '"line\nfeed"']) {
    throws(() => selectedFeatureInvocationRanges(`/run(${literal})`, [descriptor]), SyntaxError);
  }
});

Deno.test("argument completion is cursor-aware and typed invocation round-trips to input", () => {
  const named = activeInvocationArgument('/run("x", mo', 12, [descriptor]);
  assert(named, "named argument completion should be active");
  assertEquals(named.argument, null);
  assertEquals(named.prefix, "mo");

  const value = activeInvocationArgument('/run("x", mode=sa', 18, [descriptor]);
  assert(value?.argument, "enum value completion should resolve its descriptor");
  assertEquals(value.argument.name, "mode");
  assertEquals(value.prefix, "sa");

  const invocation = selectedFeatureInvocationRanges('/run("a b", mode=fast)', [descriptor])[0]
    .invocation;
  assertEquals(invocationInput(invocation), '/run(path="a b", mode="fast")');
});

Deno.test("complete invalid calls reject unknown, duplicate, missing, and mistyped arguments", () => {
  for (const input of [
    '/run(other="x")',
    '/run("x", path="y")',
    "/run()",
    '/run("x", mode=invalid)',
  ]) {
    let rejected = false;
    try {
      selectedFeatureInvocationRanges(input, [descriptor]);
    } catch {
      rejected = true;
    }
    assert(rejected, `${input} should be rejected`);
  }
});

Deno.test("a leading slash without a selected descriptor terminates without promotion", () => {
  assertEquals(activeInvocationArgument("/ru", 3, []), null);
  assertEquals(activeInvocationArgument("/unknown(", 9, [descriptor]), null);
});

Deno.test("integer invocation arguments reject unsafe JavaScript wire values", () => {
  const integerDescriptor: FeatureInvocationDescriptor = { ...descriptor,
    arguments: [{ name: "count", position: 0, required: true, value_type: { kind: "integer" }, completion: { kind: "none" } }] };
  let rejected = false;
  try { selectedFeatureInvocationRanges("/run(9007199254740992)", [integerDescriptor]); }
  catch { rejected = true; }
  assert(rejected, "unsafe integers must reject before sending to Rust");
});

Deno.test("structured invocation-only Submit and mixed source order retain exact typed payloads", async () => {
  const { buildComposerSegmentsRequest } = await import("./composer-command.ts");
  const invocation = finalizeSelectedFeatureInvocations([{ kind: "text", content: '/run("a b/資料")' }], [descriptor])[0];
  const segments = [{ kind: "text" as const, content: "before " }, invocation, { kind: "text" as const, content: " after" }];
  const result = buildComposerSegmentsRequest(segments);
  assert(result.ok && result.request, "mixed invocation should build a Submit");
  assertEquals(result.request.segments, segments);
  const only = buildComposerSegmentsRequest([invocation]);
  assert(only.ok && only.request?.segments?.[0].kind === "feature_invoke", "invocation-only Submit must not become empty text");
});

Deno.test("positional completion after a named-only argument uses positional rather than comma index", () => {
  const input = '/run(mode=safe, "';
  assertEquals(activeInvocationArgument(input, input.length, [descriptor])?.argument?.name, "path");
});

Deno.test("Feature string parsing decodes full JSON escapes and preserves round-trip Unicode", () => {
  const raw = String.raw`/run("a\b\f\n\r\t\"\\\/\u0000\uD83D\uDE00")`;
  const parsed = selectedFeatureInvocationRanges(raw, [descriptor])[0].invocation;
  assertEquals(parsed.arguments[0].value, { kind: "string", value: 'a\b\f\n\r\t"\\/\u0000😀' });
  const restored = selectedFeatureInvocationRanges(invocationInput(parsed), [descriptor])[0].invocation;
  assertEquals(restored.arguments, parsed.arguments);
});

Deno.test("Feature integer lexical syntax matches negative-or-positive digits only", () => {
  const integerDescriptor: FeatureInvocationDescriptor = { ...descriptor,
    arguments: [{ name: "count", position: 0, required: true, value_type: { kind: "integer" }, completion: { kind: "none" } }] };
  for (const [raw, value] of [["-0", 0], ["0012", 12], ["-0012", -12]] as const) {
    assertEquals(selectedFeatureInvocationRanges(`/run(${raw})`, [integerDescriptor])[0].invocation.arguments[0].value,
      { kind: "integer", value });
  }
  for (const raw of ["+1", "1.0", "1e0", "NaN", "Infinity"]) {
    let rejected = false;
    try { selectedFeatureInvocationRanges(`/run(${raw})`, [integerDescriptor]); } catch { rejected = true; }
    assert(rejected, `${raw} must reject`);
  }
});

Deno.test("invocation integer parsing matches the generated Rust int32 schema", () => {
  const integerDescriptor: FeatureInvocationDescriptor = { ...descriptor,
    arguments: [{ name: "count", position: 0, required: true, value_type: { kind: "integer" }, completion: { kind: "none" } }] };
  for (const value of [-2147483648, 2147483647]) {
    assertEquals(selectedFeatureInvocationRanges(`/run(${value})`, [integerDescriptor])[0].invocation.arguments[0].value,
      { kind: "integer", value });
  }
  for (const value of [-2147483649, 2147483648]) {
    let rejected = false;
    try { selectedFeatureInvocationRanges(`/run(${value})`, [integerDescriptor]); } catch { rejected = true; }
    assert(rejected, "values outside Rust int32 range must reject");
  }
});
