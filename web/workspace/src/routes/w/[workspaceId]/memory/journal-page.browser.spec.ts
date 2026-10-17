// @vitest-environment happy-dom
import { cleanup, render, screen, within } from "@testing-library/svelte";
import { afterEach, expect, test } from "vitest";
import JournalPage from "./[subjectId]/journal/PageContent.svelte";
import MemoryPage from "./[subjectId]/[memoryId]/PageContent.svelte";

afterEach(cleanup);
const result = <T>(data: T | null, error: string | null = null) => ({
  data,
  error,
});
const entry = (id = "entry-1", title = "Passkeyへ統一した") => ({
  id,
  journal_id: "journal-1",
  position: 2,
  state: "fixed",
  title,
  correction_refs: [],
  content_token: "content-1",
  created_at: "2026-10-16T12:00:00Z",
  updated_at: "2026-10-16T12:00:00Z",
  fixed_at: "2026-10-16T12:00:00Z",
});
const metadata = {
  id: "journal-1",
  subject_id: "subject-1",
  source_session_id: "deleted-session",
  title: "認証方式を整理したSession",
  current_position_bytes: 10,
  process_order: 4,
  process_byte: 10,
  state_token: "state-1",
  created_at: "2026-10-16T12:00:00Z",
  updated_at: "2026-10-16T12:00:00Z",
};
const source = { state: "missing", history_href: null };
const base = {
  workspaceId: "workspace-1",
  subjectId: "subject-1",
  journalId: null,
  entryId: null,
  query: null,
  view: "entries",
  after: null,
  urlQuery: "",
  mode: "list",
  sessions: null,
  entries: result({
    items: [entry(), {
      ...entry("entry-2", "別Sessionの出来事"),
      journal_id: "journal-2",
    }],
    next: "page-2",
  }),
  position: null,
  detail: null,
};
const text = (body: string) =>
  result({ text: body, next_byte: null, content_token: "token-1" });
function detail() {
  return {
    ...base,
    mode: "detail",
    journalId: "journal-1",
    entryId: "entry-1",
    urlQuery: "journal=journal-1&entry=entry-1",
    detail: {
      entry: result({
        journal: metadata,
        entry: entry(),
        original_source: source,
      }),
      body: text("保存された本文は原履歴から独立しています。"),
      ongoing: text(""),
      position: text("現在の位置"),
      targets: [{
        ...entry("earlier", "訂正される判断"),
        journal_id: "earlier-journal",
      }],
      targetNext: null,
      corrections: result({
        items: [entry("later", "後から修正した")],
        next: null,
      }),
      related: result({
        items: [{
          claim: "復旧経路を先に確認する",
          related: {
            selector: {
              memory_id: "memory-1",
              change_id: "change-1",
              candidate_id: "candidate-1",
            },
            source_index: 0,
            reconsideration: true,
          },
        }],
        next: {
          memory_id: "memory-1",
          change_id: "change-1",
          candidate_id: "candidate-1",
          source_index: 1,
        },
      }),
    },
  };
}

test("recent entries preserve literal search/filter/cursor in navigation and directly identify each entry", () => {
  render(
    JournalPage,
    {
      data: {
        ...base,
        query: "認証 & 復旧",
        urlQuery: "q=%E8%AA%8D%E8%A8%BC+%26+%E5%BE%A9%E6%97%A7",
      },
    } as never,
  );
  expect((screen.getByRole("searchbox") as HTMLInputElement).value).toBe(
    "認証 & 復旧",
  );
  expect(
    screen.getByRole("link", { name: /別Sessionの出来事/ }).getAttribute(
      "href",
    ),
  ).toContain("journal=journal-2&entry=entry-2");
  const next = new URL(
    screen.getByRole("link", { name: "次のページ →" }).getAttribute("href")!,
    "https://example.com",
  );
  expect(next.searchParams.get("q")).toBe("認証 & 復旧");
  expect(next.searchParams.get("after")).toBe("page-2");
});

test.each([
  [
    { ...base, entries: result({ items: [], next: null }) },
    "保存済みの出来事はありません。",
    false,
  ],
  [
    { ...base, query: "not-found", entries: result({ items: [], next: null }) },
    "検索結果は0件です。",
    false,
  ],
  [
    { ...base, entries: result(null, "HTTP 403") },
    "出来事一覧を取得できません。HTTP 403",
    true,
  ],
  [
    { ...base, view: "sessions", sessions: result({ items: [], next: null }) },
    "Journalはまだありません。",
    false,
  ],
])(
  "empty, zero search, denied and no Journal remain distinct",
  (data, message, alert) => {
    render(JournalPage, { data } as never);
    expect(screen.getByText(message, { exact: false })).not.toBeNull();
    expect(screen.queryByRole("alert") !== null).toBe(alert);
    expect(screen.queryByText("記録待ち・未反映の原資料があります。"))
      .toBeNull();
  },
);

test("sessions display confirmed unreflected source state, not a fabricated recording completion", () => {
  render(JournalPage, {
    data: {
      ...base,
      view: "sessions",
      sessions: result({
        items: [{
          journal: metadata,
          unreflected_sources: true,
          original_source: source,
        }, {
          journal: { ...metadata, id: "journal-2", title: "別のSession" },
          unreflected_sources: false,
          original_source: { state: "unavailable", history_href: null },
        }],
        next: "sessions-2",
      }),
    },
  } as never);
  expect(screen.getByText("記録待ち・未反映の原資料があります。")).not
    .toBeNull();
  expect(screen.getByText("保存された原資料に未反映は確認されていません。")).not
    .toBeNull();
  expect(
    screen.getByRole("link", { name: "別のSession →" }).getAttribute("href"),
  ).toContain("journal=journal-2");
  expect(screen.getByText("元の会話は削除済み")).not.toBeNull();
  expect(screen.getByText(/原資料は取得不能/)).not.toBeNull();
});

test("deleted originals do not hide saved entry, correction links or titled immutable Memory links", () => {
  render(JournalPage, { data: detail() } as never);
  expect(screen.getByText("保存された本文は原履歴から独立しています。")).not
    .toBeNull();
  expect(screen.getByRole("heading", { name: "Sessionの現在地" })).not
    .toBeNull();
  expect(
    screen.getByRole("link", { name: "復旧経路を先に確認する →" }).getAttribute(
      "href",
    ),
  ).toBe("/w/workspace-1/memory/subject-1/memory-1?change_id=change-1");
  expect(
    screen.getByRole("link", { name: "復旧経路を先に確認する →" }).getAttribute(
      "target",
    ),
  ).toBeNull();
  expect(
    screen.getByRole("link", { name: "訂正される判断 →" }).getAttribute("href"),
  ).toContain("journal=earlier-journal&entry=earlier");
  expect(
    screen.getByRole("link", { name: /後から修正した/ }).getAttribute("href"),
  ).toContain("entry=later");
  expect(screen.getByText("元の会話は削除済み")).not.toBeNull();
  expect(
    screen.getByRole("link", { name: /すべて見る（関連Memory/ }).getAttribute(
      "href",
    ),
  ).toContain("related_after=");
  expect(screen.queryByText(/source Journal/)).toBeNull();
});

test("an unavailable related API is not treated as unconnected and does not replace saved body", () => {
  const data = detail();
  data.detail.related = result(null, "HTTP 403") as never;
  render(JournalPage, { data } as never);
  expect(screen.getByRole("alert").textContent).toContain(
    "関連するMemoryを取得できません。",
  );
  expect(screen.getByText("保存された本文は原履歴から独立しています。")).not
    .toBeNull();
  expect(screen.queryByText("関連するMemoryは未接続です。")).toBeNull();
});

test("Memory shows dated entry links separately from original sources and limits preview via explicit continuation", () => {
  const selector = {
    memory_id: "memory-1",
    change_id: "change-1",
    candidate_id: "candidate-1",
  };
  const linked = {
    link: {
      source_index: 0,
      original_source: "missing",
      connection: {
        state: "linked",
        journal_id: "journal-1",
        entry_id: "entry-1",
        target: "available",
        reconsideration: false,
      },
    },
    entry: entry(),
    original_source: source,
  };
  render(MemoryPage, {
    data: {
      workspaceId: "workspace-1",
      subjectId: "subject-1",
      memoryId: "memory-1",
      subject: result(null),
      changes: result({ items: [], has_more: false }),
      memory: result({
        memory_id: "memory-1",
        change_id: "change-1",
        current_change_id: "change-1",
        claim: "復旧経路の学び",
        kind: "lesson",
        state: "active",
        body_md: "原履歴から抽出したMemory",
        source_candidates: [],
        derived_from: [],
        created_at: entry().created_at,
        updated_at: entry().updated_at,
      }),
      journalSources: [{
        candidateId: "candidate-1",
        result: result({
          selector,
          sources: [linked, {
            ...linked,
            link: {
              ...linked.link,
              source_index: 1,
              connection: { state: "unconnected", reason: "not_reconciled" },
            },
            entry: null,
            original_source: { state: "unavailable", history_href: null },
          }],
          total_sources: 5,
          next_source_index: 2,
        }),
      }],
      journalCandidateNext: null,
      journalQuery: "change_id=change-1",
    },
  } as never);
  const related = screen.getByRole("region", { name: "関連するJournal" });
  const link = within(related).getByRole("link", { name: /Passkeyへ統一した/ });
  expect(link.textContent).toMatch(/2026|10/);
  expect(link.getAttribute("href")).toContain(
    "journal=journal-1&entry=entry-1",
  );
  expect(link.getAttribute("target")).toBeNull();
  expect(within(related).getByText("対応するJournalは未接続")).not.toBeNull();
  expect(within(related).queryByText("元の会話は削除済み")).toBeNull();
  const original = screen.getByRole("region", { name: "原資料" });
  expect(within(original).getByText("元の会話は削除済み")).not.toBeNull();
  expect(within(original).getByText(/原資料は取得不能/)).not.toBeNull();
  expect(
    within(related).getByRole("link", { name: /すべて見る/ }).getAttribute(
      "href",
    ),
  ).toContain("journal_source_offset=2");
  expect(screen.queryByText(/記録待ち/)).toBeNull();
});
