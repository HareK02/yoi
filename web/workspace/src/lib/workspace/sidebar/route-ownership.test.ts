declare const Deno: {
  test(name: string, fn: () => void): void;
};

import { ownsRoutePath } from "./route-ownership.ts";

function assertEquals(actual: unknown, expected: unknown): void {
  if (!Object.is(actual, expected)) {
    throw new Error(`Expected ${String(expected)}, got ${String(actual)}`);
  }
}

Deno.test("route ownership includes only the base route and its descendants", () => {
  assertEquals(
    ownsRoutePath(
      "/design-lab/workspace-web-ux",
      "/design-lab/workspace-web-ux",
    ),
    true,
  );
  assertEquals(
    ownsRoutePath(
      "/design-lab/workspace-web-ux",
      "/design-lab/workspace-web-ux/settings",
    ),
    true,
  );
  assertEquals(
    ownsRoutePath(
      "/design-lab/workspace-web-ux",
      "/w/0197a949-4b6b-7f2a-9d9a-1f87e3a4c5b6/workers",
    ),
    false,
  );
});

Deno.test("route ownership rejects prefix collisions and empty authorities", () => {
  assertEquals(ownsRoutePath("/w/alpha", "/w/alphabet/workers"), false);
  assertEquals(
    ownsRoutePath(
      "/design-lab/workspace-web-ux/settings",
      "/design-lab/workspace-web-ux/settings-preview",
    ),
    false,
  );
  assertEquals(ownsRoutePath("", "/w/alpha"), false);
  assertEquals(ownsRoutePath("/w/alpha", ""), false);
});
