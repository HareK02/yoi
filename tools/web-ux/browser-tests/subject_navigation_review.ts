/** SubjectのIA移行を実production buildと隔離HTTPで検証する。Backend認可の証拠ではない。 */
import { assert, assertEquals } from "@std/assert";
import { dirname, fromFileUrl, join, resolve } from "@std/path";
import { chromium, type Page } from "playwright";
import { sha256File } from "../src/artifacts.ts";
import { startOwnedProcesses, stopOwnedProcesses } from "../src/processes.ts";
import { freePort } from "./memory_document_harness.ts";

const root = resolve(dirname(fromFileUrl(import.meta.url)), "../../..");
const [buildArg, runId] = Deno.args;
if (!buildArg || !runId || !/^[a-zA-Z0-9._-]+$/.test(runId)) {
  throw new Error(
    "usage: subject_navigation_review.ts <actual-build> <unique-run-id>",
  );
}
const build = resolve(buildArg);
await Deno.stat(join(build, "index.html"));
const output = join(root, "target/web-ux/t817", runId);
await Deno.mkdir(output, { recursive: true });
const port = await freePort(), base = `http://127.0.0.1:${port}`;
const ws = "/w/memory-review", subjects = `${ws}/subjektiv`;
const subject =
  "release-coordination-subject-with-a-deliberately-long-stable-identity-for-responsive-review";
const detail = `${subjects}/${subject}`;
const memory =
  "memory-decision-with-a-deliberately-long-stable-identity-for-overflow-review-0000000001";
const captures: Record<string, unknown>[] = [],
  failures: string[] = [],
  faults: Record<string, unknown>[] = [];
const manifest = {
  evidence: "actual-static-Web-build / synthetic-HTTP / NOT-real-Backend-auth-or-Runtime",
  source: new TextDecoder().decode(
    (await new Deno.Command("git", { args: ["rev-parse", "HEAD"], cwd: root })
      .output()).stdout,
  ).trim(),
  buildIndexSha256: await sha256File(join(build, "index.html")),
  runnerSha256: await sha256File(fromFileUrl(import.meta.url)),
  browser: "",
  captures,
  faults,
  failures,
  status: "running",
  visualVerdict: "UNREVIEWED",
};
const save = () =>
  Deno.writeTextFile(
    join(output, "review-context.json"),
    JSON.stringify(manifest, null, 2),
  );
const owned = await startOwnedProcesses(
  [{
    id: "navigation-synthetic-http",
    command: Deno.execPath(),
    cwd: root,
    args: [
      "run",
      "--config",
      "tools/web-ux/deno.json",
      "-A",
      "tools/web-ux/browser-tests/memory_document_fixture_server.ts",
      String(port),
      build,
      "3",
      "navigation",
    ],
    readyUrl: `${base}/health`,
  }],
  join(output, "scenario.json"),
  join(output, "logs"),
  [],
);
const browser = await chromium.launch({ headless: true });
manifest.browser = browser.version();
try {
  for (const colorScheme of ["light", "dark"] as const) {
    for (const width of [1440, 768, 390]) {
      const context = await browser.newContext({
        colorScheme,
        viewport: { width, height: 900 },
        reducedMotion: "reduce",
      });
      const page = await context.newPage();
      page.setDefaultTimeout(15000);
      let expectedFailure = false;
      const requests: string[] = [];
      page.on("request", (r) => requests.push(new URL(r.url()).pathname));
      page.on(
        "pageerror",
        (e) =>
          faults.push({
            kind: "pageerror",
            message: e.message,
            expected: false,
          }),
      );
      page.on("console", (m) => {
        if (m.type() === "error") {
          faults.push({
            kind: "console",
            message: m.text(),
            expected: expectedFailure,
          });
        }
      });
      page.on(
        "requestfailed",
        (r) =>
          faults.push({
            kind: "request",
            path: new URL(r.url()).pathname,
            message: r.failure()?.errorText,
            expected: r.failure()?.errorText === "net::ERR_ABORTED",
          }),
      );
      page.on("response", (r) => {
        if (r.status() >= 400) {
          faults.push({
            kind: "http",
            path: new URL(r.url()).pathname,
            status: r.status(),
            expected: expectedFailure,
          });
        }
      });
      // 公開chat APIの外側だけを合成。会話UI・loader・routerは本番のまま。
      await context.route(
        "**/api/w/memory-review/chat/conversations**",
        (route) =>
          route.fulfill({
            json: new URL(route.request().url()).pathname.endsWith("/conversations")
              ? {
                id: "conversation",
                subject_id: "empty-subject",
                participant_account_id: "owner",
                last_user_message_id: null,
              }
              : {
                conversation: {
                  id: "conversation",
                  subject_id: "empty-subject",
                  participant_account_id: "owner",
                  last_user_message_id: null,
                },
                messages: [],
                has_more: false,
              },
          }),
      );
      async function direct(path: string, selector: string, canonical = path) {
        const response = await page.goto(base + path);
        assertEquals(response?.status(), 200);
        assert(response?.headers()["content-type"].includes("text/html"));
        await page.waitForURL(base + canonical);
        await page.locator(selector).waitFor();
        assertEquals(
          new URL(page.url()).pathname + new URL(page.url()).search +
            new URL(page.url()).hash,
          canonical,
        );
      }
      async function selected(label = "Subjects") {
        const nav = page.getByRole("navigation", {
          name: "Workspace sections",
          exact: true,
          includeHidden: true,
        });
        assertEquals(
          await nav.getByRole("link", {
            name: label,
            exact: true,
            includeHidden: true,
          }).getAttribute("aria-current"),
          "page",
        );
        const other = label === "Subjects" ? "Staging" : "Subjects";
        assertEquals(
          await nav.getByRole("link", {
            name: other,
            exact: true,
            includeHidden: true,
          }).getAttribute("aria-current"),
          null,
        );
      }
      async function capture(id: string) {
        if (id === "detail" || id === "chat") {
          await page.locator(".subject-body .actions").waitFor();
          assertEquals(await page.locator(".subject-body [role=alert]").count(), 0);
        }
        const image = `${colorScheme}-${width}-${id}.png`;
        await page.screenshot({ path: join(output, image) });
        const overflow = await page.evaluate(
          "document.documentElement.scrollWidth > innerWidth",
        );
        assertEquals(overflow, false, image);
        const legacyLinks = await page.locator(
          "main a[href], .sidebar-sections a[href], .workspace-breadcrumbs a[href]",
        ).evaluateAll((links) =>
          links.map((l) => l.getAttribute("href")!).filter((h) =>
            /\/memory(?:\/|$)/.test(h) && !h.includes("/memory/staging")
          )
        );
        assertEquals(legacyLinks, []);
        captures.push({
          image,
          sha256: await sha256File(join(output, image)),
          route: page.url(),
          colorScheme,
          width,
          mainWidth: await page.locator("main").evaluate((el) => el.clientWidth),
          overflow,
          alerts: await page.getByRole("alert").allTextContents(),
          snapshot: await page.locator("body").ariaSnapshot(),
        });
        await save();
      }
      try {
        await direct(subjects, '[data-memory-view="subjects"]');
        await selected();
        await capture("list");
        if (width === 390) {
          await page.getByRole("button", { name: "Show sidebar", exact: true })
            .click();
          await page.getByRole("navigation", {
            name: "Workspace sections",
            exact: true,
          }).waitFor();
          await capture("mobile-navigation");
          await page.getByRole("button", { name: "Hide sidebar", exact: true })
            .click();
        }
        await page.locator(`a.subject-row[href="${detail}"]`).click();
        await page.locator(".subject-overview").waitFor();
        assertEquals(new URL(page.url()).pathname, detail);
        await selected();
        await capture("detail");
        const breadcrumbs = page.getByRole("navigation", {
          name: "Current workspace location",
        });
        assertEquals(
          await breadcrumbs.getByRole("link", { name: "Subjects", exact: true })
            .getAttribute("href"),
          subjects,
        );
        await page.getByRole("link", { name: /Journal ·/ }).click();
        await page.locator('[data-journal-view="list"] .entry-list').waitFor();
        assertEquals(new URL(page.url()).pathname, detail + "/journal");
        await capture("journal");
        await page.goBack();
        await page.locator(".subject-overview").waitFor();
        await page.goForward();
        await page.locator('[data-journal-view="list"]').waitFor();
        await direct(
          `${detail}/memories/${memory}?change_id=change-earlier#related-journal`,
          '[data-memory-view="detail"]',
        );
        assertEquals(
          await page.locator('.workspace-breadcrumbs a[href$="/memories"]')
            .count(),
          0,
        );
        await capture("memory-history");
        await direct(
          subjects + "/permissions-edit/permissions",
          "#permissions-heading",
        );
        await page.getByRole("combobox", { name: "Operation", exact: true })
          .waitFor();
        await selected();
        await capture("permissions-owner");
        await direct(
          subjects + "/permissions-reader/permissions",
          "#permissions-heading",
        );
        await page.getByRole("status").filter({ hasText: "Read-only." })
          .waitFor();
        await capture("permissions-reader");
        await direct(
          subjects + "/empty-subject/chat",
          ".conversation-page textarea",
        );
        await page.getByText("No public messages yet.").waitFor();
        await selected();
        await capture("chat");
        await direct(`${ws}/memory/staging`, ".staging-summary-grid");
        await selected("Staging");
        await capture("staging");
        await direct(subjects + "/staging", ".subject-overview");
        await selected();
        const encoded = encodeURIComponent("encoded/Subject ?#日本%");
        await direct(
          `${ws}/memory/${encoded}?cursor=a%2Fb%26c#subject-heading`,
          ".subject-overview",
          `${subjects}/${encoded}?cursor=a%2Fb%26c#subject-heading`,
        );
        assert(
          requests.includes(
            `/api/w/memory-review/subjektiv/subjects/${encoded}`,
          ),
        );
        await page.reload();
        await page.locator(".subject-overview").waitFor();
        // 旧URLの曖昧性はstatic予約語優先。新memories namespaceで同名IDを読める。
        for (
          const id of [
            "permissions",
            "journal",
            "chat",
            "conversation",
            "a/b ?#%",
          ]
        ) {
          await direct(
            `${detail}/memories/${encodeURIComponent(id)}`,
            '[data-memory-view="detail"]',
          );
        }
        for (
          const [old, next, selector] of [
            [
              `${ws}/memory?cursor=a%2Fb#memory-subjects-heading`,
              `${subjects}?cursor=a%2Fb#memory-subjects-heading`,
              '[data-memory-view="subjects"]',
            ],
            [
              `${ws}/memory/${subject}?cursor=a%2Fb#subject-heading`,
              `${detail}?cursor=a%2Fb#subject-heading`,
              ".subject-overview",
            ],
            [
              `${ws}/memory/${subject}/${memory}?change_id=change-earlier#related-journal`,
              `${detail}/memories/${memory}?change_id=change-earlier#related-journal`,
              '[data-memory-view="detail"]',
            ],
            [
              `${ws}/memory/${subject}/journal?journal=journal-release#journal-title`,
              `${detail}/journal?journal=journal-release#journal-title`,
              "[data-journal-view]",
            ],
            [
              `${ws}/memory/permissions-edit/permissions#permissions-heading`,
              `${subjects}/permissions-edit/permissions#permissions-heading`,
              "#permissions-heading",
            ],
            [
              `${ws}/memory/empty-subject/conversation?from=bookmark#conversation-subject-heading`,
              `${subjects}/empty-subject/chat?from=bookmark#conversation-subject-heading`,
              ".conversation-page textarea",
            ],
            [
              `${ws}/memory/empty-subject/chat`,
              `${subjects}/empty-subject/chat`,
              ".conversation-page textarea",
            ],
          ]
        ) await direct(old, selector, next);
        // replace遷移は旧entryを残さず、戻る/進むで同じURLを往復しない。
        await direct(subjects, '[data-memory-view="subjects"]');
        await direct(`${ws}/memory/${subject}`, ".subject-overview", detail);
        await page.getByRole("link", { name: /Journal ·/ }).click();
        await page.locator("[data-journal-view]").waitFor();
        await page.goBack();
        await page.waitForURL(base + detail);
        await page.goBack();
        await page.waitForURL(base + subjects);
        await page.goForward();
        await page.waitForURL(base + detail);
        const beforeUnknown = requests.length;
        await direct(
          `${ws}/memory/empty-subject/unknown/deeper`,
          ".legacy-subject-route",
        );
        await page.getByText(
          "This legacy path has no corresponding Subject page.",
        ).waitFor();
        assertEquals(
          requests.slice(beforeUnknown).filter((p) => p.includes("/subjektiv/subjects/")),
          [],
        );
        // 読取り拒否を空一覧/新権限付与へ変えず、UIがBackend失敗をそのまま示す。
        expectedFailure = true;
        await context.route(
          "**/subjektiv/subjects/empty-subject",
          (route) =>
            route.fulfill({
              status: 403,
              json: { error: "Forbidden", message: "synthetic read denied" },
            }),
        );
        await direct(
          subjects + "/empty-subject/chat",
          '.conversation-page [role="alert"]',
        );
        await capture("chat-denied");
        expectedFailure = false;
      } catch (e) {
        failures.push(
          `${colorScheme}-${width}: ${e instanceof Error ? e.stack : e}`,
        );
        await page.screenshot({
          path: join(output, `${colorScheme}-${width}-failure.png`),
        });
        throw e;
      } finally {
        await context.close();
        await save();
      }
    }
  }
  assertEquals(failures, []);
  assertEquals(faults.filter((f) => !f.expected), []);
  manifest.status = "passed-automated-not-visual-verdict";
} finally {
  await browser.close();
  await stopOwnedProcesses(owned);
  await save();
}
console.log(
  JSON.stringify({
    output,
    status: manifest.status,
    captures: captures.length,
  }),
);
