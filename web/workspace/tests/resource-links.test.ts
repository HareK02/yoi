// @ts-nocheck
import {
  canonicalResourceReference,
  resourceHumanKey,
  slugifyResourceTitle,
} from "../src/lib/workspace/resource-links.ts";

function assertEquals(actual: unknown, expected: unknown): void {
  if (actual !== expected) {
    throw new Error(`expected ${String(expected)}, received ${String(actual)}`);
  }
}

Deno.test("resource links normalize titles and preserve the human key", () => {
  assertEquals(slugifyResourceTitle(" Fix stale URL / 日本語 "), "fix-stale-url-日本語");
  assertEquals(
    canonicalResourceReference("T-1842", "Fix stale URL / 日本語"),
    "T-1842-fix-stale-url-日本語",
  );
  assertEquals(resourceHumanKey("T-1842-fix-stale-url-日本語"), "T-1842");
});

Deno.test("resource links use a deterministic fallback for punctuation-only titles", () => {
  assertEquals(canonicalResourceReference("O-7", "---"), "O-7-resource");
  assertEquals(resourceHumanKey("01a017internal"), "01a017internal");
});
