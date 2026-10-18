/** 本番static web buildとloopback HTTP fixtureでUI境界だけを検証する。
 * 最新のweb/workspace/buildが前提で、このsuiteはbuildしない。
 * 製品・Worker・modelは起動せず、Journal保存・認可の保証はBackend試験が担う。
 * tools/web-uxのtest:browserまたはこのfileを明示選択して実行する。
 */
import { assert, assertEquals } from "@std/assert";
import { dirname, fromFileUrl, join } from "@std/path";
import { chromium, type Locator, type Page } from "playwright";
import { journalFixture as fixture } from "./journal_browser_fixture.ts";
import {
  assertMainOwnsVerticalScroll,
  assertNoPageWideOverflow,
  freePort,
  mainScrollState,
  waitForServer,
} from "./memory_document_harness.ts";

const root = join(dirname(fromFileUrl(import.meta.url)), "../../..");
const artifacts = join(root, "target/web-ux/t-737-browser-interactions");
const subjectPath = `/w/${fixture.workspaceId}/memory/${fixture.subjectId}`;
const journalPath = `${subjectPath}/journal`;
const memoryPath =
  `${subjectPath}/${fixture.memoryId}?change_id=change-current`;
const claim = "Keep subject Memory provenance typed and visible";
const entryTitle = "Release milestone 00 · release [a+b] %_";
const detailPath = `${journalPath}?journal=journal-release&entry=entry-00`;

async function ordinaryAnchor(link: Locator) {
  assertEquals(await link.evaluate((el) => el.tagName), "A");
  assertEquals(await link.getAttribute("target"), null);
  assert(!(await link.textContent())?.includes("↗"));
  assert((await link.getAttribute("href"))?.startsWith("/w/"));
}
async function restoreTop(page: Page, expected: number) {
  await page.waitForFunction(
    (top) => {
      const main = (globalThis as any).document.querySelector("main");
      return main && Math.abs(main.scrollTop - top) <= 3;
    },
    expected,
    { timeout: 10_000 },
  );
  assert(Math.abs((await mainScrollState(page)).scrollTop - expected) <= 3);
}
async function capture(page: Page, name: string) {
  await assertNoPageWideOverflow(page);
  await assertMainOwnsVerticalScroll(page);
  await page.screenshot({ path: join(artifacts, `${name}.png`) });
}

Deno.test("Journal fixture UI keeps scoped navigation, bounded links and distinct read states", async (t) => {
  await Deno.stat(join(root, "web/workspace/build/index.html"));
  await Deno.mkdir(artifacts, { recursive: true });
  const port = await freePort();
  const base = `http://127.0.0.1:${port}`;
  const server = new Deno.Command(Deno.execPath(), {
    args: [
      "run",
      "--allow-net",
      "--allow-read",
      join(
        root,
        "tools/web-ux/browser-tests/memory_document_fixture_server.ts",
      ),
      String(port),
      join(root, "web/workspace/build"),
    ],
    stdout: "null",
    stderr: "null",
  }).spawn();
  try {
    await waitForServer(`${base}/health`);
    const browser = await chromium.launch({ headless: true });
    const requests: {
      path: string;
      method: string;
      payload: Record<string, any> | null;
    }[] = [];
    const errors: string[] = [];
    try {
      const context = await browser.newContext({
        viewport: { width: 1440, height: 900 },
      });
      context.on("request", (request) => {
        const path = new URL(request.url()).pathname;
        if (path.startsWith("/api/")) {
          requests.push({
            path,
            method: request.method(),
            payload: request.method() === "POST"
              ? request.postDataJSON()
              : null,
          });
        }
      });
      context.on(
        "page",
        (page) => page.on("pageerror", (e) => errors.push(String(e))),
      );
      const page = await context.newPage();
      await t.step(
        "Subject entry and multiple Sessions retain pending/source state and continuation",
        async () => {
          await page.goto(`${base}${subjectPath}`);
          const entry = page.getByRole("link", {
            name: "Journal · 最近の出来事とSessionを探す →",
            exact: true,
          });
          await entry.waitFor();
          await ordinaryAnchor(entry);
          await entry.click();
          await page.locator("[data-journal-view='list']").waitFor();
          await page.getByRole("link", {
            name: "SessionごとのJournal",
            exact: true,
          }).click();
          await page.getByRole("link", {
            name: "Release coordination Session →",
            exact: true,
          })
            .waitFor();
          assertEquals(await page.locator(".session-list > li").count(), 2);
          await page.getByText("記録待ち・未反映の原資料があります。", {
            exact: true,
          }).waitFor();
          await capture(page, "sessions-desktop-light");
          const next = page.getByRole("link", {
            name: "次のページ →",
            exact: true,
          });
          await next.click();
          await page.getByRole("link", {
            name: "Archived planning Session →",
            exact: true,
          })
            .waitFor();
          assertEquals(
            new URL(page.url()).searchParams.get("view"),
            "sessions",
          );
          assertEquals(
            new URL(page.url()).searchParams.get("after"),
            "fixture-sessions-2",
          );
          await page.getByRole("link", { name: "一覧の先頭へ", exact: true })
            .click();
          await page.getByRole("link", {
            name: "Release coordination Session →",
            exact: true,
          })
            .click();
          await page.getByRole("heading", { name: "Current Session position" })
            .waitFor();
          await page.getByRole("heading", { name: "出来事の経緯", exact: true })
            .waitFor();
          assert((await page.locator(".entry-list > li").count()) > 1);
          assertEquals(
            new URL(page.url()).searchParams.get("journal"),
            "journal-release",
          );
        },
      );

      await t.step(
        "literal search and bounded continuation preserve the actual URL query",
        async () => {
          await page.goto(`${base}${journalPath}`);
          await page.getByRole("searchbox", { name: "出来事を検索" }).fill(
            fixture.literalQuery,
          );
          await page.getByRole("button", { name: "検索", exact: true }).click();
          await page.waitForURL((url) =>
            url.searchParams.get("q") === fixture.literalQuery
          );
          await page.getByRole("link", {
            name: new RegExp(entryTitle.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")),
          }).waitFor();
          assertEquals(await page.locator(".entry-list > li").count(), 16);
          await page.getByRole("link", { name: "次のページ →", exact: true })
            .click();
          await page.waitForURL((url) =>
            url.searchParams.get("after") === "fixture-entries-2"
          );
          assertEquals(
            new URL(page.url()).searchParams.get("q"),
            fixture.literalQuery,
          );
          assertEquals(
            await page.getByRole("searchbox").inputValue(),
            fixture.literalQuery,
          );
          await page.getByText("Release milestone 16 · release [a+b] %_", {
            exact: true,
          })
            .waitFor();
          assertEquals(await page.locator(".entry-list > li").count(), 12);
          const searchRequests = requests.filter((r) =>
            r.payload?.action === "entries" &&
            r.payload.query === fixture.literalQuery
          );
          assert(
            searchRequests.some((r) =>
              r.payload?.after === "fixture-entries-2"
            ),
          );
        },
      );

      for (
        const viewport of [
          { width: 1440, height: 900, theme: "light" as const },
          { width: 768, height: 900, theme: "dark" as const },
          { width: 390, height: 844, theme: "light" as const },
        ]
      ) {
        await t.step(
          `list → entry → Memory → Back → Back restores main scroll at ${viewport.width}`,
          async () => {
            await page.setViewportSize(viewport);
            await page.emulateMedia({ colorScheme: viewport.theme });
            await page.goto(
              `${base}${journalPath}?q=${
                encodeURIComponent(fixture.literalQuery)
              }`,
            );
            const target = page.locator(".entry-list a").filter({
              hasText: "Release milestone 12",
            });
            await target.waitFor();
            await target.scrollIntoViewIfNeeded();
            const listTop = (await mainScrollState(page)).scrollTop;
            assert(
              listTop > 100,
              "list must actually be scrolled before testing restoration",
            );
            const listUrl = page.url();
            await ordinaryAnchor(target);
            await target.click();
            await page.getByRole("heading", {
              name: "Release milestone 12 · release [a+b] %_",
              exact: true,
            }).waitFor();
            const memoryLink = page.locator(".related-list").getByRole("link", {
              name: `${claim} →`,
              exact: true,
            });
            await memoryLink.scrollIntoViewIfNeeded();
            const detailTop = (await mainScrollState(page)).scrollTop;
            assert(
              detailTop > 100,
              "detail must actually be scrolled before testing restoration",
            );
            const detailUrl = page.url();
            await capture(
              page,
              `detail-before-memory-${viewport.width}-${viewport.theme}`,
            );
            await ordinaryAnchor(memoryLink);
            assertEquals(await memoryLink.getAttribute("href"), memoryPath);
            await memoryLink.click();
            await page.getByRole("heading", { name: claim, exact: true })
              .waitFor();
            assertEquals(
              new URL(page.url()).pathname,
              `${subjectPath}/${fixture.memoryId}`,
            );
            assertEquals(
              new URL(page.url()).searchParams.get("change_id"),
              "change-current",
            );
            const journalLinks = page.locator(
              "#related-journal .journal-link-list a",
            );
            assertEquals(await journalLinks.count(), 3);
            const journalLink = journalLinks.filter({ hasText: entryTitle });
            await ordinaryAnchor(journalLink);
            assertEquals(await journalLink.getAttribute("href"), detailPath);
            assert(
              (await journalLink.textContent())!.length > entryTitle.length + 4,
              "Journal link includes date as well as title",
            );
            await journalLink.scrollIntoViewIfNeeded();
            await capture(
              page,
              `memory-related-${viewport.width}-${viewport.theme}`,
            );
            await page.goBack();
            await page.locator("[data-journal-view='detail']").waitFor();
            assertEquals(page.url(), detailUrl);
            await restoreTop(page, detailTop);
            await page.goBack();
            await page.getByRole("searchbox").waitFor();
            assertEquals(page.url(), listUrl);
            assertEquals(
              await page.getByRole("searchbox").inputValue(),
              fixture.literalQuery,
            );
            await restoreTop(page, listTop);
            await capture(
              page,
              `list-back-${viewport.width}-${viewport.theme}`,
            );
          },
        );
      }

      await t.step(
        "related previews, immutable candidates, normal same-tab and new-tab links",
        async () => {
          await page.setViewportSize({ width: 1440, height: 900 });
          await page.goto(`${base}${detailPath}`);
          await page.getByRole("heading", { name: entryTitle, exact: true })
            .waitFor();
          assertEquals(await page.locator(".related-list a").count(), 3);
          await page.getByRole("link", {
            name: "すべて見る（関連Memoryの続き） →",
            exact: true,
          })
            .click();
          await page.getByText("Related release lesson 3 →", { exact: true })
            .waitFor();
          assertEquals(await page.locator(".related-list a").count(), 5);
          assertEquals(new URL(page.url()).searchParams.get("related"), "all");
          assert(new URL(page.url()).searchParams.has("related_after"));
          await page.locator(".related-list").scrollIntoViewIfNeeded();
          await capture(page, "journal-all-related-1440-light");
          await page.getByRole("link", {
            name: "関連Memoryの先頭へ",
            exact: true,
          }).click();
          const memoryLink = page.locator(".related-list a").filter({
            hasText: claim,
          });
          const popupReady = context.waitForEvent("page");
          await memoryLink.click({ button: "middle" });
          const popup = await popupReady;
          await popup.getByRole("heading", { name: claim, exact: true })
            .waitFor();
          assertEquals(
            new URL(popup.url()).searchParams.get("change_id"),
            "change-current",
          );
          const backLink = popup.locator("#related-journal a").filter({
            hasText: entryTitle,
          });
          const journalPopupReady = context.waitForEvent("page");
          await backLink.click({ button: "middle" });
          const journalPopup = await journalPopupReady;
          await journalPopup.getByRole("heading", {
            name: entryTitle,
            exact: true,
          }).waitFor();
          await journalPopup.getByRole("heading", {
            name: "Preserved Journal narrative",
            exact: true,
          }).waitFor();
          await journalPopup.close();
          await popup.getByRole("link", {
            name: "すべて見る（関連Journalの続き） →",
            exact: true,
          })
            .click();
          await popup.locator("#related-journal a").filter({
            hasText: "Release milestone 03",
          })
            .waitFor();
          assertEquals(
            await popup.locator("#related-journal .journal-link-list a")
              .count(),
            5,
          );
          assertEquals(
            new URL(popup.url()).searchParams.get("journal_source_offset"),
            "3",
          );
          await popup.locator("#related-journal").scrollIntoViewIfNeeded();
          await capture(popup, "memory-all-related-1440-light");
          await popup.locator("#related-journal a").filter({
            hasText: "Release milestone 03",
          })
            .click();
          await popup.getByRole("heading", {
            name: "Release milestone 03 · release [a+b] %_",
            exact: true,
          }).waitFor();
          assertEquals(
            new URL(popup.url()).searchParams.get("entry"),
            "entry-03",
          );
          await popup.getByText("関連するMemoryは未接続です。", { exact: true })
            .waitFor();
          assertEquals(await popup.locator(".related-list a").count(), 0);
          await popup.close();
          const candidateRequests = requests.filter((r) =>
            r.payload?.action === "memory_sources"
          );
          assert(candidateRequests.length > 0);
          for (const r of candidateRequests) {
            assertEquals(r.payload?.selector.candidate_id, fixture.candidateId);
            assertEquals(r.payload?.selector.change_id, "change-current");
            assertEquals(r.payload?.selector.memory_id, fixture.memoryId);
          }
        },
      );

      await t.step(
        "bounded narrative continuation carries its content token and recovers from stale text",
        async () => {
          await page.goto(`${base}${detailPath}`);
          const body = page.getByRole("region", {
            name: "出来事の本文",
            exact: true,
          });
          await body.getByRole("link", { name: "次の本文区間 →", exact: true })
            .click();
          await page.getByRole("heading", {
            name: "Journal narrative complete",
            exact: true,
          })
            .waitFor();
          assertEquals(
            new URL(page.url()).searchParams.get("body_token"),
            "content-entry-0",
          );
          assert(Number(new URL(page.url()).searchParams.get("body_byte")) > 0);
          await body.getByRole("link", { name: "本文の先頭へ", exact: true })
            .click();
          await page.getByRole("heading", {
            name: "Preserved Journal narrative",
            exact: true,
          })
            .waitFor();
          await page.goto(
            `${base}${detailPath}&body_byte=8192&body_token=stale-content-token`,
          );
          await body.getByRole("alert").waitFor();
          await body.getByRole("link", {
            name: "先頭から読み直す",
            exact: true,
          }).click();
          await page.getByRole("heading", {
            name: "Preserved Journal narrative",
            exact: true,
          })
            .waitFor();
        },
      );

      await t.step(
        "correction targets and correcting-entry links resolve directly",
        async () => {
          await page.goto(`${base}${detailPath}`);
          const correction = page.getByRole("link").filter({
            hasText: "Corrected release evidence",
          });
          await correction.waitFor();
          await ordinaryAnchor(correction);
          await correction.click();
          await page.getByRole("heading", {
            name: "Corrected release evidence",
            exact: true,
          })
            .waitFor();
          const target = page.getByRole("link", {
            name: `${entryTitle} →`,
            exact: true,
          });
          await ordinaryAnchor(target);
          assertEquals(await target.getAttribute("href"), detailPath);
          await target.click();
          await page.getByRole("heading", { name: entryTitle, exact: true })
            .waitFor();
          await page.getByRole("region", { name: "継続中の経緯", exact: true })
            .waitFor();
        },
      );

      await t.step(
        "missing and unavailable original sources do not prevent saved Journal reading",
        async () => {
          for (
            const state of [
              {
                journal: "journal-release",
                entry: "entry-00",
                text: "元の会話は削除済み",
              },
              {
                journal: "journal-review",
                entry: "entry-01",
                text: "原資料は取得不能（存在・閲覧可否を確認できません）",
              },
            ]
          ) {
            await page.goto(
              `${base}${journalPath}?journal=${state.journal}&entry=${state.entry}`,
            );
            await page.getByRole("heading", {
              name: "Preserved Journal narrative",
              exact: true,
            })
              .waitFor();
            await page.getByRole("region", { name: "原資料", exact: true })
              .getByText(state.text, {
                exact: true,
              }).waitFor();
            assertEquals(
              await page.getByRole("link", {
                name: "元の会話を開く →",
                exact: true,
              }).count(),
              0,
            );
            assertEquals(await page.locator(".related-list a").count(), 3);
          }
          await page.goto(
            `${base}${journalPath}?journal=journal-planning&entry=entry-04`,
          );
          await page.getByText("原資料の本文は未確認です。", { exact: true })
            .waitFor();
          const originalLink = page.getByRole("link", {
            name: "元の会話を確認する →",
            exact: true,
          });
          await ordinaryAnchor(originalLink);
          assertEquals(
            await originalLink.getAttribute("href"),
            "/w/memory-review/workers/W-fixture/console",
          );
          await originalLink.scrollIntoViewIfNeeded();
          await capture(page, "retained-original-navigation-1440-light");
        },
      );

      await t.step(
        "empty, pending, unconnected, no search result and request failures remain distinct",
        async () => {
          for (
            const state of [
              {
                path:
                  `/w/${fixture.workspaceId}/memory/empty-subject/journal?view=sessions`,
                text: "Journalはまだありません。",
                role: "status" as const,
              },
              {
                path: `${journalPath}?q=definitely-no-such-literal`,
                text: "検索結果は0件です。",
                role: "status" as const,
              },
              {
                path: `${journalPath}?q=fixture-failure`,
                text: "出来事一覧を取得できません。",
                role: "alert" as const,
              },
              {
                path: `${journalPath}?journal=journal-denied&entry=entry-00`,
                text: "Journalエントリを取得できません",
                role: "alert" as const,
              },
            ]
          ) {
            await page.goto(`${base}${state.path}`);
            await page.getByRole(state.role).filter({ hasText: state.text })
              .waitFor();
            assertEquals(
              await page.locator(".entry-list a, .related-list a").count(),
              0,
            );
            await assertNoPageWideOverflow(page);
          }
          await page.goto(
            `${base}${subjectPath}/memory-resolved-0002?change_id=change-earlier`,
          );
          await page.locator("#related-journal").getByText(
            "対応するJournalは未接続",
            {
              exact: true,
            },
          ).waitFor();
          assertEquals(await page.locator("#related-journal a").count(), 0);
          assert(
            !(await page.locator("#related-journal").textContent())?.includes(
              "記録待ち",
            ),
          );
          assert(requests.some((r) =>
            r.payload?.action === "memory_sources" &&
            r.payload.selector.memory_id === "memory-resolved-0002" &&
            r.payload.selector.change_id === "change-earlier" &&
            r.payload.selector.candidate_id === fixture.candidateId
          ));
          await page.goto(
            `${base}${subjectPath}/memory-sources-failure?change_id=change-current`,
          );
          await page.getByRole("alert").filter({
            hasText: "Journal参照を取得できません。",
          })
            .waitFor();
          await page.getByRole("article", {
            name: "Committed Memory body",
            exact: true,
          }).waitFor();
          assertEquals(
            await page.locator("#related-journal .journal-link-list a").count(),
            0,
          );
        },
      );
      assertEquals(errors, []);
      assertEquals(
        requests.filter((r) =>
          r.method !== "GET" &&
          !(r.method === "POST" && r.path.endsWith("/journal-browser"))
        ),
        [],
      );
      assertEquals(
        requests.filter((r) => /history|restore|start/.test(r.path)),
        [],
      );
      for (
        const request of requests.filter((r) =>
          r.path.endsWith("/journal-browser")
        )
      ) {
        assertEquals(request.method, "POST");
        if (request.payload?.limit != null) {
          assert(request.payload.limit > 0 && request.payload.limit <= 20);
        }
        if (request.payload?.action === "text") {
          assert(
            request.payload.max_bytes > 0 && request.payload.max_bytes <= 8192,
          );
        }
      }
      await Deno.writeTextFile(
        join(artifacts, "requests.json"),
        JSON.stringify(requests, null, 2),
      );
      await context.close();
    } finally {
      await browser.close();
    }
  } finally {
    server.kill("SIGTERM");
    await server.status;
  }
});
