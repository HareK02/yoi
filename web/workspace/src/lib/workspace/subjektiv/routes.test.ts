declare const Deno: { test(name: string, fn: () => void): void };
function assertEquals(actual: unknown, expected: unknown): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `Expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}
import {
  legacySubjectDestination,
  subjectChatHref,
  subjectHref,
  subjectMemoryHref,
  subjectsHref,
} from "./routes.ts";
import { buildWorkspaceBreadcrumbs } from "../header/breadcrumb-model.ts";
import { ownsRoutePath } from "../sidebar/route-ownership.ts";

Deno.test("Subject routes encode opaque identities once and namespace Memory IDs", () => {
  assertEquals(subjectsHref("ws /&"), "/w/ws%20%2F%26/subjektiv");
  assertEquals(subjectHref("w", "staging"), "/w/w/subjektiv/staging");
  assertEquals(
    subjectChatHref("w", "a/b ?#日本%"),
    "/w/w/subjektiv/a%2Fb%20%3F%23%E6%97%A5%E6%9C%AC%25/chat",
  );
  for (
    const id of ["journal", "permissions", "chat", "conversation", "a/b ?#%"]
  ) {
    assertEquals(
      subjectMemoryHref("w", "s", id),
      `/w/w/subjektiv/s/memories/${encodeURIComponent(id)}`,
    );
  }
});

Deno.test("legacy migration preserves query/hash and static ownership without API lookup", () => {
  for (
    const [old, canonical] of [
      ["", ""],
      ["/", ""],
      ["/s", "/s"],
      ["/s/", "/s"],
      ["/s/permissions", "/s/permissions"],
      ["/s/journal", "/s/journal"],
      ["/s/chat", "/s/chat"],
      ["/s/conversation", "/s/chat"],
      ["/s/m-1", "/s/memories/m-1"],
      ["/a%2Fb/m%252F", "/a%2Fb/memories/m%252F"],
      ["/s/memories", "/s/memories/memories"],
      ["/staging/journal", "/staging/journal"],
      ["/staging/m-1", "/staging/memories/m-1"],
    ]
  ) {
    const url = new URL(
      `https://example.test/w/ws%2Fa/memory${old}?cursor=a%2Fb+%26&change_id=%252F#memory%2Fsection`,
    );
    const expected = `/w/ws%2Fa/subjektiv${canonical}${url.search}${url.hash}`;
    assertEquals(legacySubjectDestination(url), expected);
    assertEquals(legacySubjectDestination(new URL(expected, url)), null);
  }
  for (
    const path of [
      "/w/w/memory/staging",
      "/w/w/memory/staging/",
      "/w/w/memory/staging/unknown/deeper",
      "/w/w/memory/s/x/y",
      "/w/w/memory/s//x",
      "/w/w/memory/%zz",
      "/w/w/memories/s",
      "/w/w/subjektiv/s",
    ]
  ) {
    assertEquals(
      legacySubjectDestination(new URL(path, "https://example.test")),
      null,
    );
  }
});

Deno.test("Subject sidebar ownership and breadcrumbs never lead to legacy or nonexistent Memory index", () => {
  const root = subjectsHref("w");
  assertEquals(ownsRoutePath(root, "/w/w/subjektiv/s/chat"), true);
  for (
    const path of [
      "/w/w/subjektiv-other",
      "/w/w/memory/staging",
      "/w/other/subjektiv",
    ]
  ) {
    assertEquals(ownsRoutePath(root, path), false);
  }
  assertEquals(buildWorkspaceBreadcrumbs("/w/w/memory/staging", "w"), [{
    label: "Memory",
  }, { label: "Staging" }]);
  assertEquals(buildWorkspaceBreadcrumbs(root, "w"), [{ label: "Subjects" }]);
  assertEquals(
    buildWorkspaceBreadcrumbs("/w/w/subjektiv/a%2Fb/memories/permissions", "w"),
    [
      { label: "Subjects", href: root },
      { label: "a/b", href: "/w/w/subjektiv/a%2Fb" },
      { label: "permissions" },
    ],
  );
  for (
    const [route, label] of [["chat", "Conversation"], ["journal", "Journal"], [
      "permissions",
      "Operation permissions",
    ]]
  ) {
    assertEquals(
      buildWorkspaceBreadcrumbs(`/w/w/subjektiv/staging/${route}`, "w"),
      [
        { label: "Subjects", href: root },
        { label: "staging", href: "/w/w/subjektiv/staging" },
        { label },
      ],
    );
  }
});
