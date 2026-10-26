/// <reference lib="dom" />
// Native Chromium touch input against the production shell; HTTP uses the auth fixture.
import { assertEquals } from "@std/assert";
import type { Browser } from "playwright";

export async function sidebarSwipe(
  t: Deno.TestContext,
  browser: Browser,
  origin: string,
) {
  for (
    const scenario of [
      "right",
      "vertical",
      "diagonal",
      "short",
      "horizontal-scroll",
      "input",
      "desktop",
      "anonymous",
      "close",
      "close-vertical",
      "close-diagonal",
      "close-short",
      "close-horizontal-scroll",
      "close-input",
      "close-opposite",
    ]
  ) {
    await t.step(`native touch swipe: ${scenario}`, async () => {
      const context = await browser.newContext({
        viewport: { width: scenario === "desktop" ? 1200 : 390, height: 844 },
        hasTouch: true,
        isMobile: scenario !== "desktop",
        reducedMotion: "reduce",
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
        const closing = scenario.startsWith("close");
        const kind = closing ? scenario.slice(6) : scenario;
        if (closing) {
          await page.getByRole("button", { name: "Show sidebar", exact: true })
            .click();
        }
        const surface = closing ? ".sidebar-frame-content" : "main";
        // Supply deterministic scroll/control surfaces inside the actual shell.
        await page.locator(surface).evaluate((main) => {
          const area = document.createElement("div");
          area.innerHTML =
            '<div id="swipe-plain" style="height:180px">Swipe surface</div><div id="swipe-horizontal" style="overflow-x:auto;width:280px"><div style="width:900px;height:100px">Horizontal content</div></div><input id="swipe-input" type="range" style="width:280px"><div style="height:1800px"></div>';
          main.prepend(area);
        });
        const target = kind === "horizontal-scroll"
          ? "#swipe-horizontal"
          : kind === "input"
          ? "#swipe-input"
          : "#swipe-plain";
        const box = (await page.locator(target).boundingBox())!;
        if (kind === "horizontal-scroll") {
          await page.locator(target).evaluate((node) => {
            node.scrollLeft = 200;
          });
        }
        await page.evaluate(() =>
          new Promise<void>((resolve) =>
            requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
          )
        );
        const client = await context.newCDPSession(page);
        const startX = box.x + (closing ? 210 : 60);
        const startY = box.y + (kind === "vertical" ? 150 : box.height / 2);
        const dx = (closing && kind !== "opposite" ? -1 : 1) *
          (kind === "vertical" ? 8 : kind === "short" ? 40 : 150);
        const dy = kind === "vertical" ? -130 : kind === "diagonal" ? 100 : 3;
        await client.send("Input.dispatchTouchEvent", {
          type: "touchStart",
          touchPoints: [{ x: startX, y: startY, id: 1 }],
        });
        for (let i = 1; i <= 8; i++) {
          await client.send("Input.dispatchTouchEvent", {
            type: "touchMove",
            touchPoints: [{
              x: startX + dx * i / 8,
              y: startY + dy * i / 8,
              id: 1,
            }],
          });
        }
        await client.send("Input.dispatchTouchEvent", {
          type: "touchEnd",
          touchPoints: [],
        });
        await page.evaluate(() =>
          new Promise<void>((resolve) =>
            requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
          )
        );
        if (scenario === "right") {
          await page.getByRole("button", { name: "Hide sidebar", exact: true })
            .waitFor();
          assertEquals(await page.locator("main").getAttribute("inert"), "");
          await page.getByRole("button", { name: "Hide sidebar", exact: true })
            .click();
          await page.getByRole("button", { name: "Show sidebar", exact: true })
            .waitFor();
        } else if (scenario === "close") {
          await page.getByRole("button", { name: "Show sidebar", exact: true })
            .waitFor();
          assertEquals(await page.locator("main").getAttribute("inert"), null);
          await page.getByRole("button", {
            name: "Add passkey to alice",
            exact: true,
          }).waitFor();
        } else {
          assertEquals(
            await page.locator(".app-shell.sidebar-open").count(),
            closing ? 1 : 0,
          );
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
                  ? document.querySelector("#swipe-horizontal")!.scrollLeft >
                    200
                  : document.querySelector("#swipe-horizontal")!.scrollLeft <
                    200,
              closing,
            );
          }
          if (scenario === "anonymous") {
            assertEquals(await page.locator(".sidebar-frame").count(), 0);
          }
        }
        assertEquals(errors, []);
      } finally {
        await context.close();
      }
    });
  }
}
