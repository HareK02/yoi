/// <reference lib="dom" />
// Native Chromium touch input against the production shell; HTTP uses the auth fixture.
import { assert, assertEquals } from "@std/assert";
import type { Browser, Page } from "playwright";

async function painted(page: Page) {
  await page.evaluate(() =>
    new Promise<void>((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
    )
  );
}

export async function sidebarSwipe(
  t: Deno.TestContext,
  browser: Browser,
  origin: string,
) {
  const kinds = [
    "drag",
    "flick",
    "slow",
    "short",
    "arc",
    "ambiguous",
    "return",
    "reverse",
    "vertical",
    "diagonal",
    "horizontal-scroll",
    "horizontal-edge",
    "input",
    "button",
    "button-short",
    "link",
    "edge",
    "cancel",
    "multitouch",
    "resize",
  ];
  for (
    const scenario of [
      ...kinds,
      ...kinds.map((kind) => `close-${kind}`),
      "desktop",
      "anonymous",
    ]
  ) {
    await t.step(`native touch swipe: ${scenario}`, async () => {
      const context = await browser.newContext({
        viewport: { width: scenario === "desktop" ? 1200 : 390, height: 844 },
        hasTouch: true,
        isMobile: scenario !== "desktop",
        reducedMotion: scenario.endsWith("drag") ? "no-preference" : "reduce",
      });
      try {
        const page = await context.newPage();
        page.setDefaultTimeout(10_000);
        const errors: string[] = [];
        page.on("pageerror", (error) => errors.push(error.message));
        await page.request.get(
          `${origin}/?fixture=${
            scenario === "anonymous" ? "anonymous" : "account"
          }`,
        );
        await page.goto(
          `${origin}${scenario === "anonymous" ? "/" : "/account"}`,
        );
        await page.getByRole("heading", {
          name: scenario === "anonymous" ? "Yoi" : "Account",
          exact: true,
        }).waitFor();
        if (scenario === "desktop") {
          await page.getByRole("button", { name: "Unpin sidebar" }).click();
        }
        const closing = scenario.startsWith("close-");
        const kind = closing ? scenario.slice(6) : scenario;
        if (closing) {
          await page.getByRole("button", { name: "Show sidebar", exact: true })
            .click();
          await page.waitForFunction(() =>
            Math.abs(
              document.querySelector(".sidebar-frame")!.getBoundingClientRect()
                .left,
            ) < 1
          );
        }
        const surface = closing ? ".sidebar-frame-content" : "main";
        await page.locator(surface).evaluate((main) => {
          const area = document.createElement("div");
          area.innerHTML =
            '<div id="swipe-plain" style="height:180px">Swipe surface</div><div id="swipe-horizontal" style="overflow-x:auto;width:280px"><div style="width:900px;height:100px">Horizontal content</div></div><input id="swipe-input" type="range" style="width:280px"><button id="swipe-button" style="display:block;width:280px;height:70px">Ordinary button</button><a id="swipe-link" href="#unexpected" style="display:block;width:280px;height:70px">Ordinary link</a><div style="height:1800px"></div>';
          for (const control of area.querySelectorAll("button, a")) {
            control.addEventListener("click", () => {
              area.dataset.clicked = "true";
            });
          }
          area.id = "swipe-fixture";
          main.prepend(area);
        });
        const target = kind.startsWith("horizontal-")
          ? "#swipe-horizontal"
          : kind === "input"
          ? "#swipe-input"
          : kind.startsWith("button")
          ? "#swipe-button"
          : kind === "link"
          ? "#swipe-link"
          : "#swipe-plain";
        if (kind === "horizontal-scroll") {
          await page.locator(target).evaluate((node) => {
            node.scrollLeft = 200;
          });
        }
        await painted(page);
        const box = (await page.locator(target).boundingBox())!;
        const startX = kind === "edge"
          ? (closing ? 380 : 10)
          : box.x + (closing ? 245 : 35);
        const startY = box.y + (kind === "vertical" ? 150 : box.height / 2);
        const direction = closing ? -1 : 1;
        // dx is measured toward the requested state. Explicit event timestamps
        // make flick/hold tests independent of CDP roundtrip speed.
        let points = [[20, 1, 40], [80, 2, 160], [200, 3, 400]];
        if (kind === "flick") points = [[12, 1, 15], [40, 2, 50]];
        if (kind === "slow") {
          points = [[20, 1, 400], [80, 2, 900], [200, 3, 1600]];
        }
        if (kind === "short" || kind === "button-short") {
          points = [[20, 1, 200], [45, 2, 400]];
        }
        if (kind === "arc") {
          points = [[20, 2, 40], [80, 35, 160], [200, 65, 400]];
        }
        if (kind === "ambiguous") {
          points = [[8, 7, 20], [25, 10, 60], [200, 15, 400]];
        }
        if (kind === "return") {
          points = [[20, 1, 40], [200, 2, 200], [5, 3, 600]];
        }
        if (kind === "reverse") {
          // Stationary CDP points do not emit touchmove. Sample both moving
          // points of the reversal so its release velocity is observable.
          points = [[20, 1, 40], [250, 2, 200], [230, 2, 530], [180, 3, 560]];
        }
        if (kind === "vertical") {
          points = [[2, -20, 40], [4, -70, 160], [8, -130, 400]];
        }
        if (kind === "diagonal") {
          points = [[15, -15, 40], [50, -50, 160], [100, -100, 400]];
        }
        const client = await context.newCDPSession(page);
        const base = Date.now() / 1000;
        await client.send("Input.dispatchTouchEvent", {
          type: "touchStart",
          timestamp: base,
          touchPoints: [{ x: startX, y: startY, id: 1 }],
        });
        for (const [dx, dy, time] of points) {
          await client.send("Input.dispatchTouchEvent", {
            type: "touchMove",
            timestamp: base + time / 1000,
            touchPoints: [{
              x: startX + dx * direction,
              y: startY + dy,
              id: 1,
            }],
          });
        }
        const [dx, dy, time] = points.at(-1)!;
        const claimed = ![
          "vertical",
          "diagonal",
          "horizontal-scroll",
          "horizontal-edge",
          "input",
          "edge",
          "desktop",
          "anonymous",
        ].includes(kind);
        await painted(page);
        if (claimed) {
          assertEquals(
            await page.locator(".sidebar-swiping").count(),
            1,
            `${scenario}: preview before release`,
          );
          const actual = await page.locator(".sidebar-frame").evaluate((
            panel,
          ) => ({
            x: panel.getBoundingClientRect().left,
            width: panel.getBoundingClientRect().width,
            visibility: getComputedStyle(panel).visibility,
            transition: getComputedStyle(panel).transitionDuration,
          }));
          assert(
            Math.abs(actual.x - (closing ? -dx : -actual.width + dx)) < 2,
            `${scenario}: panel must track finger, got ${actual.x}`,
          );
          assertEquals(actual.visibility, "visible");
          assertEquals(actual.transition, "0s");
          assertEquals(
            await page.locator("main").getAttribute("inert"),
            closing ? "" : null,
          );
          assertEquals(
            await page.locator(".sidebar-frame-content").getAttribute("inert"),
            closing ? null : "",
          );
        } else {
          assertEquals(await page.locator(".sidebar-swiping").count(), 0);
        }
        if (kind === "cancel") {
          await client.send("Input.dispatchTouchEvent", {
            type: "touchCancel",
            touchPoints: [],
          });
        } else {
          if (kind === "multitouch") {
            await client.send("Input.dispatchTouchEvent", {
              type: "touchStart",
              timestamp: base + (time + 5) / 1000,
              touchPoints: [
                { x: startX + dx * direction, y: startY + dy, id: 1 },
                { x: startX + dx * direction + 30, y: startY + dy, id: 2 },
              ],
            });
          }
          if (kind === "resize") {
            await page.setViewportSize({ width: 400, height: 844 });
          }
          await client.send("Input.dispatchTouchEvent", {
            type: "touchEnd",
            timestamp: base + (time + 10) / 1000,
            touchPoints: [],
          });
        }
        const commits = [
          "drag",
          "flick",
          "slow",
          "arc",
          "ambiguous",
          "button",
          "link",
        ].includes(kind);
        const expectedOpen = commits ? !closing : closing;
        await painted(page);
        if (scenario !== "desktop" && scenario !== "anonymous") {
          assertEquals(
            await page.locator(".app-shell.sidebar-open").count(),
            expectedOpen ? 1 : 0,
            `${scenario}: final state`,
          );
          assertEquals(
            await page.locator("main").getAttribute("inert"),
            expectedOpen ? "" : null,
          );
          await page.waitForFunction((open) => {
            const bounds = document.querySelector(".sidebar-frame")!
              .getBoundingClientRect();
            return Math.abs(bounds.left - (open ? 0 : -bounds.width)) < 1;
          }, expectedOpen);
        }
        assertEquals(await page.locator(".sidebar-swiping").count(), 0);
        assertEquals(
          await page.locator("#swipe-fixture").getAttribute("data-clicked"),
          null,
        );
        assertEquals(new URL(page.url()).hash, "");
        if (kind === "vertical") {
          await page.waitForFunction(
            (selector) =>
              (document.querySelector(selector)?.scrollTop ?? 0) > 0,
            surface,
          );
        }
        if (kind === "horizontal-scroll") {
          await page.waitForFunction(
            (closing) =>
              closing
                ? document.querySelector("#swipe-horizontal")!.scrollLeft > 200
                : document.querySelector("#swipe-horizontal")!.scrollLeft < 200,
            closing,
          );
        }
        if (kind === "input") {
          assert(
            (await page.locator(target).inputValue()) !== "50",
            "native range control remains operable",
          );
        }
        if (kind === "button-short") {
          // A new tap after snap-back must still activate the same button.
          await page.locator(target).tap();
          assertEquals(
            await page.locator("#swipe-fixture").getAttribute("data-clicked"),
            "true",
          );
        }
        if (scenario === "anonymous") {
          assertEquals(await page.locator(".sidebar-frame").count(), 0);
        }
        assertEquals(errors, []);
      } finally {
        await context.close();
      }
    });
  }
}
