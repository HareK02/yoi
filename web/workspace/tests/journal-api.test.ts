declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};
function assertEquals(actual: unknown, expected: unknown): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `Expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}
function assertThrows(fn: () => void): void {
  try {
    fn();
  } catch {
    return;
  }
  throw new Error("Expected rejection");
}
import {
  boundedParam,
  journalHref,
  loadJournal,
  offsetParam,
  parseJournalResponse,
} from "../src/lib/workspace/memory/journal-api.ts";

// transport/parserの局所契約。認可と保存接続はRustの実router/store試験が所有する。
const selector = {
  memory_id: "memory",
  change_id: "change",
  candidate_id: "candidate",
};
const entry = {
  id: "entry",
  journal_id: "journal",
  position: 1,
  state: "fixed",
  title: "認証方式を整理した",
  correction_refs: [],
  content_token: "token",
  created_at: "2026-10-16T00:00:00Z",
  updated_at: "2026-10-16T00:00:00Z",
  fixed_at: null,
};
const journal = {
  id: "journal",
  subject_id: "subject",
  source_session_id: "session",
  title: "認証のSession",
  current_position_bytes: 0,
  process_order: 1,
  process_byte: 0,
  state_token: "token",
  created_at: entry.created_at,
  updated_at: entry.updated_at,
};
const original = { state: "missing", history_href: null };
Deno.test("Journal parser preserves missing originals independently of linked saved entries", () => {
  const value = {
    selector,
    sources: [{
      link: {
        source_index: 0,
        original_source: "missing",
        connection: {
          state: "linked",
          journal_id: "journal",
          entry_id: "entry",
          target: "available",
          reconsideration: true,
        },
      },
      entry,
      original_source: original,
    }],
    total_sources: 8,
    next_source_index: 1,
  };
  const parsed = parseJournalResponse(
    { kind: "memory_sources", value },
    "memory_sources",
  );
  assertEquals(parsed.sources[0].entry?.title, entry.title);
  assertEquals(parsed.sources[0].original_source.state, "missing");
  assertEquals(parsed.next_source_index, 1);
  assertThrows(() =>
    parseJournalResponse({
      kind: "memory_sources",
      value: {
        ...value,
        sources: [{ ...value.sources[0], entry: { ...entry, id: "other" } }],
      },
    }, "memory_sources")
  );
});
Deno.test("Journal parser rejects wrong Subject, response kind, unbounded lists and unsafe history hrefs", () => {
  const value = {
    journal,
    entry,
    correction_targets: [{
      ...entry,
      journal_id: "another-journal",
      id: "another-entry",
    }],
    original_source: original,
  };
  assertEquals(
    parseJournalResponse({ kind: "entry", value }, "entry", "subject")
      .correction_targets[0].journal_id,
    "another-journal",
  );
  assertThrows(() =>
    parseJournalResponse({ kind: "entry", value }, "entry", "other-subject")
  );
  assertThrows(() => parseJournalResponse({ kind: "text", value }, "entry"));
  assertThrows(() =>
    parseJournalResponse({
      kind: "entries",
      value: { items: Array(33).fill(entry), next: null },
    }, "entries")
  );
  assertThrows(() =>
    parseJournalResponse({
      kind: "entry",
      value: {
        ...value,
        original_source: {
          state: "available",
          history_href: "javascript:alert(1)",
        },
      },
    }, "entry")
  );
  assertThrows(() =>
    parseJournalResponse({
      kind: "entry",
      value: { ...value, entry: { ...entry, state: "recorded" } },
    }, "entry")
  );
  assertThrows(() =>
    parseJournalResponse({
      kind: "text",
      value: {
        text: "x".repeat(16385),
        next_byte: null,
        content_token: "token",
      },
    }, "text")
  );
});
Deno.test("Journal transport reads immutable links without reconciling and rejects a mismatched selector", async () => {
  const requests: { path: string; request: unknown }[] = [];
  const fetcher = ((path: string, init: RequestInit) => {
    requests.push({ path, request: JSON.parse(String(init.body)) });
    return Promise.resolve(
      Response.json({
        kind: "memory_sources",
        value: {
          selector,
          sources: [],
          total_sources: 0,
          next_source_index: null,
        },
      }),
    );
  }) as typeof fetch;
  const request = {
    action: "memory_sources" as const,
    selector,
    source_offset: 3,
    limit: 20,
  };
  assertEquals(
    (await loadJournal(fetcher, "workspace", "subject", request)).error,
    null,
  );
  assertEquals(requests, [{
    path: "/api/w/workspace/subjektiv/subjects/subject/journal-browser",
    request,
  }]);
  const mismatch = await loadJournal(fetcher, "workspace", "subject", {
    ...request,
    selector: { ...selector, change_id: "other" },
  });
  assertEquals(mismatch.data, null);
  assertEquals(mismatch.error, "Journal API response is invalid");
});
Deno.test("Journal HTTP denial and transport failure remain failures, not empty pages", async () => {
  for (
    const [fetcher, status] of [
      [
        () => Promise.resolve(new Response("not authorized", { status: 403 })),
        403,
      ],
      [() => Promise.reject(new Error("offline")), undefined],
    ] as const
  ) {
    const value = await loadJournal(
      fetcher as typeof fetch,
      "workspace",
      "subject",
      {
        action: "entries",
        journal_id: null,
        query: null,
        after: null,
        limit: 20,
      },
    );
    assertEquals(value.data, null);
    assertEquals(value.status, status);
    assertEquals(value.error !== null, true);
  }
});
Deno.test("Journal links retain exact selected entry and bounded URL offsets", () => {
  const url = new URL(
    journalHref("workspace", "subject/a", {
      journal: "journal",
      entry: "entry&selected",
      q: "日本語",
    }),
    "https://example.com",
  );
  assertEquals(url.pathname, "/w/workspace/memory/subject%2Fa/journal");
  assertEquals(url.searchParams.get("entry"), "entry&selected");
  assertEquals(
    boundedParam(new URLSearchParams({ q: "あ".repeat(5) }), "q", 14),
    null,
  );
  assertEquals(offsetParam(new URLSearchParams({ offset: "-1" }), "offset"), 0);
  assertEquals(
    offsetParam(new URLSearchParams({ offset: "27" }), "offset"),
    27,
  );
});
