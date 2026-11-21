import {
  mergeRequestQuery,
  mergeRequestQueryPagePath,
} from "./merge-requests.ts";

declare const Deno: { test(name: string, fn: () => void): void };
function equal(actual: unknown, expected: unknown) {
  if (actual !== expected) {
    throw new Error(`Expected ${expected}, got ${actual}`);
  }
}

Deno.test("MR query passes Backend conditions unchanged rather than filtering a loaded page", () => {
  const query = mergeRequestQuery(
    new URLSearchParams({
      repository_key: "repo / one",
      selector_to: "refs/heads/develop",
      phase: "integration",
      selector_from: "work/source",
      ticket_ref: "T-3",
      assigned_to_self: "true",
      state: "open",
      cursor: "opaque+cursor/==",
      limit: "20",
      unrelated: "ignored",
    }),
  );
  equal(query.get("repository_key"), "repo / one");
  equal(query.get("selector_to"), "refs/heads/develop");
  equal(query.get("phase"), "integration");
  equal(query.get("assigned_to_self"), "true");
  equal(query.get("cursor"), "opaque+cursor/==");
  equal(query.has("unrelated"), false);
});

Deno.test("MR paging keeps filters and page size but first page clears only the cursor", () => {
  const query = new URLSearchParams(
    "phase=integration&repository_key=main&selector_to=develop&limit=20",
  );
  const next = new URL(
    mergeRequestQueryPagePath("my workspace", query, "opaque+next"),
    "https://example.test",
  );
  equal(next.pathname, "/w/my%20workspace/merge-requests");
  equal(next.searchParams.get("cursor"), "opaque+next");
  equal(next.searchParams.get("selector_to"), "develop");
  equal(next.searchParams.get("phase"), "integration");
  equal(next.searchParams.get("limit"), "20");
  const first = new URL(
    mergeRequestQueryPagePath("my workspace", next.searchParams, null),
    next,
  );
  equal(first.searchParams.has("cursor"), false);
  equal(first.searchParams.get("phase"), "integration");
  equal(query.has("cursor"), false);
});

Deno.test("MR empty conditions use bounded default while invalid values reach Backend validation", () => {
  const query = mergeRequestQuery(
    new URLSearchParams("phase=&repository_key=&cursor="),
  );
  equal(query.toString(), "limit=50");
  const invalid = mergeRequestQuery(
    new URLSearchParams("phase=invalid&limit=-1&assigned_to_self=true"),
  );
  equal(invalid.get("phase"), "invalid");
  equal(invalid.get("limit"), "-1");
  equal(invalid.get("assigned_to_self"), "true");
});
