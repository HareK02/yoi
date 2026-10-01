/// <reference lib="dom" />
import { assert, assertEquals } from "@std/assert";
import { dirname, fromFileUrl, join } from "@std/path";
import { chromium, type Page } from "playwright";

const repositoryRoot = join(dirname(fromFileUrl(import.meta.url)), "../../..");
const workspaceRoot = join(repositoryRoot, "web/workspace");
const fixtureServer = join(
  repositoryRoot,
  "tools/web-ux/browser-tests/console_history_fixture_server.ts",
);
const workspaceId = "console-history-review";
type MotionWindow = Window & {
  latestMotion: { from: number; to: number; duration: number }[];
};

async function freePort(): Promise<number> {
  const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
  const port = (listener.addr as Deno.NetAddr).port;
  listener.close();
  return port;
}

async function waitForServer(url: string): Promise<void> {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    try {
      const response = await fetch(url);
      await response.body?.cancel();
      if (response.ok) return;
    } catch {
      // Retry until the bounded deadline while the owned server starts.
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`fixture server did not become ready: ${url}`);
}

async function historyRequestCount(baseUrl: string): Promise<number> {
  const response = await fetch(`${baseUrl}/fixture-state`);
  const state = await response.json();
  return state.history_requests;
}

async function checkSidebarModeSwitch(page: Page, name: string): Promise<void> {
  const button = page.getByRole("button", { name, exact: true });
  await button.hover();
  await button.focus();
  const samples = await button.evaluate(async (button) => {
    const frame = button.closest<HTMLElement>(".sidebar-frame")!;
    await Promise.all(frame.getAnimations({ subtree: true }).map((animation) => animation.finished));
    const panel = frame.querySelector<HTMLElement>(".sidebar-frame__bevel")!;
    const content = frame.querySelector<HTMLElement>(".sidebar-frame-content")!;
    const main = document.querySelector<HTMLElement>(".app-shell__main")!;
    const sample = () => ({
      mainLeft: main.getBoundingClientRect().left,
      mainWidth: main.getBoundingClientRect().width,
      stationary: {
        panel: panel.getBoundingClientRect().toJSON(),
        content: content.getBoundingClientRect().toJSON(),
        button: button.getBoundingClientRect().toJSON(),
        folded: frame.classList.contains("folded"),
        unclipped: panel.contains(document.elementFromPoint(panel.getBoundingClientRect().right - 8, 40)),
        motion: panel.getAnimations({ subtree: true }).filter((animation) =>
          animation instanceof CSSTransition && ["width", "transform"].includes(animation.transitionProperty)
        ).length,
      },
    });
    const samples = [sample()];
    (button as HTMLButtonElement).click();
    const start = performance.now();
    do {
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
      samples.push(sample());
    } while (performance.now() - start < 280);
    return samples;
  });
  for (const sample of samples) {
    assertEquals(sample.stationary, samples[0].stationary, `${name} must keep the open panel stationary without slide animation`);
    assertEquals(sample.stationary.folded, false);
    assertEquals(sample.stationary.motion, 0);
    assertEquals(sample.stationary.unclipped, true);
  }
  const first = samples[0];
  const last = samples.at(-1)!;
  assert(samples.some((sample) => sample.mainLeft > Math.min(first.mainLeft, last.mainLeft) &&
    sample.mainLeft < Math.max(first.mainLeft, last.mainLeft)), "main position must animate through intermediate coordinates");
  assert(samples.some((sample) => sample.mainWidth > Math.min(first.mainWidth, last.mainWidth) &&
    sample.mainWidth < Math.max(first.mainWidth, last.mainWidth)), "main width must animate through intermediate sizes");
  await page.locator(".sidebar-fold-button").evaluate((button) => (button as HTMLButtonElement).blur());
}

async function checkSidebarSlide(page: Page, mobile: boolean, opening: boolean): Promise<void> {
  const toggle = page.locator(mobile ? ".app-shell__mobile-sidebar-toggle" : ".sidebar-hover-region");
  const result = await toggle.evaluate(async (button, { mobile, opening }) => {
    const frame = document.querySelector<HTMLElement>(".sidebar-frame")!;
    const content = frame.querySelector<HTMLElement>(".sidebar-frame-content")!;
    const main = document.querySelector<HTMLElement>(".app-shell__main")!;
    const moving = mobile ? frame : content;
    const footer = frame.querySelector<HTMLElement>(".sidebar-control-row")!;
    const pin = frame.querySelector<HTMLElement>(".sidebar-fold-button")!;
    const sample = () => ({
      footerWidth: footer.getBoundingClientRect().width,
      footerHeight: footer.getBoundingClientRect().height,
      dividerLeft: parseFloat(getComputedStyle(footer, "::before").left),
      dividerRight: parseFloat(getComputedStyle(footer, "::before").right),
      footerBorder: getComputedStyle(footer).borderTopWidth,
      pinWidth: pin.getBoundingClientRect().width,
      pinHeight: pin.getBoundingClientRect().height,
      x: moving.getBoundingClientRect().x,
      width: moving.getBoundingClientRect().width,
      frameWidth: frame.getBoundingClientRect().width,
      panelWidth: frame.querySelector(".sidebar-frame__bevel")!.getBoundingClientRect().width,
      mainWidth: main.getBoundingClientRect().width,
      mainHeight: main.getBoundingClientRect().height,
      pinLeft: frame.querySelector(".sidebar-fold-button")!.getBoundingClientRect().left - frame.getBoundingClientRect().left,
      pinBottom: frame.getBoundingClientRect().bottom - frame.querySelector(".sidebar-fold-button")!.getBoundingClientRect().bottom,
    });
    const before = sample();
    if (mobile) {
      (button as HTMLButtonElement).click();
    } else {
      button.dispatchEvent(new PointerEvent(opening ? "pointerenter" : "pointerleave", { pointerType: "mouse" }));
      if (!opening) await new Promise((resolve) => setTimeout(resolve, 190));
    }
    await new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
    const animations = frame.getAnimations({ subtree: true });
    const slides = animations.filter((animation) => animation instanceof CSSTransition &&
      ["width", "transform"].includes(animation.transitionProperty));
    const durations = slides.map((animation) => animation.effect!.getTiming().duration);
    for (const animation of animations) {
      animation.pause();
      animation.currentTime = 110;
    }
    const middle = sample();
    for (const animation of animations) animation.finish();
    await Promise.all(animations.map((animation) => animation.finished));
    return { before, middle, after: sample(), durations, contentInert: content.inert, mainInert: main.inert };
  }, { mobile, opening });
  assert(result.durations.length >= (mobile ? 1 : 2), `missing sidebar motion: ${JSON.stringify({ mobile, opening, result })}`);
  assert(result.durations.every((duration) => duration === 220));
  const { before, middle, after } = result;
  for (const sample of [before, middle, after]) {
    assert(Math.abs(sample.pinLeft - 8) <= 1, "Pin must stay at the sidebar's left edge");
    assert(Math.abs(sample.pinBottom - 8) <= 1, "Pin must stay at the sidebar's bottom edge");
    assertEquals(sample.pinWidth, 32);
    assertEquals(sample.pinHeight, 32);
    assertEquals(sample.footerHeight, 48, "footer keeps compact, equal padding");
    assertEquals(sample.dividerLeft, 8);
    assertEquals(sample.dividerRight, 8);
    assertEquals(sample.footerBorder, "0px", "divider must not touch the sidebar edges");
  }
  assert(opening ? before.x < middle.x && middle.x < after.x : before.x > middle.x && middle.x > after.x,
    `sidebar must slide horizontally through an intermediate position: ${JSON.stringify(result)}`);
  assertEquals(before.width, after.width, "sidebar contents must slide without reflowing");
  assertEquals(result.contentInert, !opening);
  assertEquals(result.mainInert, mobile && opening);
  assertEquals(before.mainWidth, after.mainWidth, "hover/touch previews must not resize the main content");
  assertEquals(before.mainHeight, after.mainHeight);
  assertEquals(after.frameWidth, before.frameWidth, "hover mode keeps a fixed rail width");
  if (!mobile) {
    const folded = opening ? before : after;
    assertEquals(folded.footerWidth, folded.footerHeight, "folded footer must be square");
    assert(opening ? before.panelWidth < middle.panelWidth && middle.panelWidth < after.panelWidth
      : before.panelWidth > middle.panelWidth && middle.panelWidth > after.panelWidth,
      "the overlay panel must expand past the rail and contract again");
  }
}

async function checkConsoleHistory(viewportHeight: number): Promise<void> {
  const build = await new Deno.Command(Deno.execPath(), {
    args: ["task", "build"],
    cwd: workspaceRoot,
    stdout: "piped",
    stderr: "piped",
  }).output();
  if (!build.success) {
    throw new Error(
      `Workspace production build failed:\n${new TextDecoder().decode(build.stderr)}`,
    );
  }

  const port = await freePort();
  const baseUrl = `http://127.0.0.1:${port}`;
  const server = new Deno.Command(Deno.execPath(), {
    args: [
      "run",
      "--allow-net",
      "--allow-read",
      fixtureServer,
      String(port),
      join(workspaceRoot, "build"),
    ],
    stdout: "null",
    stderr: "null",
  }).spawn();

  try {
    await waitForServer(`${baseUrl}/health`);
    const browser = await chromium.launch({ headless: true });
    try {
      const context = await browser.newContext({
        viewport: { width: 1440, height: viewportHeight },
        colorScheme: "dark",
      });
      const page = await context.newPage();
      page.setDefaultTimeout(5_000);
      await page.addInitScript(() => {
        (window as unknown as MotionWindow).latestMotion = [];
        const animate = Element.prototype.animate;
        Element.prototype.animate = function (frames, options) {
          if (this.matches(".console-jump-latest") && Array.isArray(frames) && frames.length > 0) {
            (window as unknown as MotionWindow).latestMotion.push({
              from: Number(frames[0].opacity),
              to: Number(frames.at(-1)?.opacity),
              duration: Number(typeof options === "number" ? options : options?.duration),
            });
          }
          return animate.call(this, frames, options);
        };
      });
      const errors: string[] = [];
      const responses: string[] = [];
      page.on("console", (message) => {
        if (message.type() === "error") errors.push(message.text());
      });
      page.on("pageerror", (error) => errors.push(String(error)));
      page.on("response", (response) => {
        if (response.status() >= 400 || response.url().includes("/api/")) {
          responses.push(`${response.status()} ${response.url()}`);
        }
      });

      await page.goto(
        `${baseUrl}/w/${workspaceId}/workers/W-900-console-fixture/console`,
      );
      const transcript = page.getByRole("article", { name: "main transcript" });
      try {
        await transcript.getByText("question 8", { exact: true }).waitFor({ timeout: 5_000 });
      } catch (error) {
        throw new Error(
          `Console did not render retained history: ${error}\nresponses=${
            JSON.stringify(responses)
          }\nerrors=${JSON.stringify(errors)}\nbody=${await page.locator("body").innerText()}`,
        );
      }
      const navigation = page.getByRole("navigation", { name: "Conversation turns" });
      await navigation.getByRole("button", { name: /^Turn \d+: question 7$/ }).waitFor();
      assertEquals(await historyRequestCount(baseUrl), 1);
      assertEquals(await transcript.getByText(/^question (7|8|9|10|11|12)$/).count(), 6);
      assertEquals(await navigation.getByRole("button", { name: /^Turn \d+: question (7|8|9|10|11|12)$/ }).count(), 6);
      assertEquals(await transcript.getByText("searched 1 time・ran 1 command", { exact: true }).count(), 6,
        "overlapping snapshot and history must render each tool activity once");
      const turnList = navigation.locator(".turn-list");
      const initialGeometry = await turnList.evaluate((element) => {
        const boundary = element.querySelector(".history-boundary")!;
        const control = boundary.firstElementChild!;
        const first = element.querySelector(".turn-button")!;
        return {
          boundaryHeight: boundary.getBoundingClientRect().height,
          controlHeight: control.getBoundingClientRect().height,
          boundaryOffset: boundary.getBoundingClientRect().top - element.getBoundingClientRect().top + element.scrollTop,
          firstOffset: first.getBoundingClientRect().top - boundary.getBoundingClientRect().top,
          expectedOffset: Math.max(0, (element.getBoundingClientRect().height - element.children.length * 24) / 2),
        };
      });
      assertEquals(initialGeometry.boundaryHeight, 24);
      assertEquals(initialGeometry.controlHeight, 24);
      assertEquals(initialGeometry.firstOffset, 24);
      assert(Math.abs(initialGeometry.boundaryOffset - initialGeometry.expectedOffset) <= 1,
        "short turn lists must be vertically centered, with overflowing lists starting at the top");
      assertEquals(await turnList.evaluate((element) => element.scrollHeight > element.clientHeight), viewportHeight === 340);
      const currentQuestionTop = await transcript
        .getByText("question 7", { exact: true })
        .evaluate((element) => element.getBoundingClientRect().top);
      const retainedQuestionTop = await transcript
        .getByText("question 8", { exact: true })
        .evaluate((element) => element.getBoundingClientRect().top);
      assert(
        currentQuestionTop < retainedQuestionTop,
        "the unmatched current-snapshot turn must precede overlapping retained history",
      );

      const question12 = navigation.getByRole("button", {
        name: /^Turn \d+: question 12$/,
      });
      await question12.scrollIntoViewIfNeeded();
      await page.waitForTimeout(50);
      await question12.hover();
      const preview = navigation.getByRole("tooltip");
      await preview.waitFor();
      assert((await preview.textContent())?.includes("detail 12.1"));
      assert((await preview.textContent())?.includes("detail 12.2"));
      assert(!(await preview.textContent())?.includes("detail 12.3"));
      await page.keyboard.press("Escape");

      const consoleScroll = transcript.locator("xpath=..");
      const topAlignment = await page.evaluate(() => {
        const document = (globalThis as any).document;
        const body = document.querySelector(".console-scroll")?.getBoundingClientRect();
        const turns = document.querySelector(".turn-navigation")?.getBoundingClientRect();
        return body && turns ? Math.abs(body.top - turns.top) : Number.POSITIVE_INFINITY;
      });
      assert(topAlignment <= 1, `transcript and turn bar top edges differ by ${topAlignment}px`);

      const navigationAnchoredTop = await navigation
        .locator('[data-turn-id="user-8"]')
        .evaluate((element) => element.getBoundingClientRect().top);
      const anchoredTop = await consoleScroll.evaluate((element) => {
        element.scrollTop = 0;
        const anchor = element.querySelector('[data-console-line-id*="user-8"]');
        const top = anchor?.getBoundingClientRect().top ?? Number.NaN;
        element.dispatchEvent(new Event("scroll"));
        return top;
      });
      await transcript.getByText("question 3", { exact: true }).waitFor();
      await navigation.getByRole("button", { name: /^Turn \d+: question 3$/ }).waitFor();
      const restoredTop = await transcript
        .locator('[data-console-line-id*="user-8"]')
        .evaluate((element) => element.getBoundingClientRect().top);
      assert(
        Math.abs(restoredTop - anchoredTop) <= 1,
        `transcript anchor moved from ${anchoredTop}px to ${restoredTop}px`,
      );
      await page.waitForTimeout(250);
      assertEquals(
        await historyRequestCount(baseUrl),
        2,
        "a stationary top sentinel must not drain another page",
      );

      const navigationAfterPrepend = await navigation
        .locator('[data-turn-id="user-8"]')
        .evaluate((element) => element.getBoundingClientRect().top);
      const overflowing = await turnList.evaluate((element) => element.scrollHeight > element.clientHeight);
      assertEquals(overflowing, viewportHeight < 600);
      if (overflowing) {
        const scroll = await turnList.evaluate((element) => ({
          top: element.scrollTop, max: element.scrollHeight - element.clientHeight,
        }));
        // When a short list first overflows, the requested anchor can exceed
        // the scroll range. Restore as far as possible without artificial space.
        const requested = scroll.top + navigationAfterPrepend - navigationAnchoredTop;
        assert(Math.abs(scroll.top - Math.max(0, Math.min(scroll.max, requested))) <= 1);
        if (viewportHeight === 340) {
          assert(
            Math.abs(navigationAfterPrepend - navigationAnchoredTop) <= 1,
            `turn-bar anchor moved from ${navigationAnchoredTop}px to ${navigationAfterPrepend}px`,
          );
        }
        await turnList.evaluate((element) => {
          element.scrollTop = Math.max(2, element.scrollTop);
          element.dispatchEvent(new Event("scroll"));
          element.scrollTop = 0;
          element.dispatchEvent(new Event("scroll"));
        });
      } else {
        // A short list cannot scroll: keep the whole compact list centered
        // rather than adding artificial space to force a fixed anchor.
        const centerOffset = await turnList.evaluate((element) => {
          const first = element.firstElementChild!.getBoundingClientRect();
          const last = element.lastElementChild!.getBoundingClientRect();
          const viewport = element.getBoundingClientRect();
          return (first.top + last.bottom - viewport.top - viewport.bottom) / 2;
        });
        assert(Math.abs(centerOffset) <= 1, "short lists must remain centered after prepending history");
        await navigation.getByRole("button", { name: "Earlier conversation available" }).click();
      }
      await transcript.getByText("question 1", { exact: true }).waitFor();
      await navigation.getByRole("button", { name: /^Turn \d+: question 1$/ }).waitFor();
      assertEquals(await historyRequestCount(baseUrl), 3);
      assertEquals(await page.getByText("Start of conversation", { exact: true }).count(), 1);

      await navigation.getByRole("button", {
        name: /^Turn \d+: question 3$/,
      }).click();
      await page.waitForFunction(() => {
        const document = (globalThis as any).document;
        const element = document.querySelector('[data-console-line-id*="user-3"]');
        const parent = element?.closest(".console-scroll");
        if (!element || !parent) return false;
        const item = element.getBoundingClientRect();
        const viewport = parent.getBoundingClientRect();
        return item.top >= viewport.top - 1 && item.top < viewport.bottom;
      });
      const jumpGeometry = await transcript
        .locator('[data-console-line-id*="user-3"]')
        .evaluate((element) => {
          const parent = element.closest(".console-scroll")!;
          const item = element.getBoundingClientRect();
          const viewport = parent.getBoundingClientRect();
          return { itemTop: item.top, viewportTop: viewport.top, viewportBottom: viewport.bottom };
        });
      assert(jumpGeometry.itemTop >= jumpGeometry.viewportTop - 1);
      assert(jumpGeometry.itemTop < jumpGeometry.viewportBottom);

      const latest = page.getByRole("button", { name: "Jump to latest", exact: true });
      await latest.waitFor();
      await latest.evaluate(async (element) => {
        await Promise.all(element.getAnimations().map((animation) => animation.finished));
      });
      await latest.hover();
      await page.waitForFunction(() => getComputedStyle(document.querySelector(".console-jump-latest")!).translate === "0px -2px");
      assert((await latest.evaluate((element) => getComputedStyle(element).transitionDuration)).includes("0.14s"));
      await page.mouse.move(0, 0);
      await latest.evaluate(async (element) => {
        await Promise.all(element.getAnimations().map((animation) => animation.finished));
      });
      assertEquals((await latest.textContent())?.trim(), "");
      const viewportBeforeJump = await consoleScroll.boundingBox();
      const latestGeometry = await latest.boundingBox();
      assert(viewportBeforeJump && latestGeometry);
      assert(Math.abs(latestGeometry.x + latestGeometry.width / 2 - viewportBeforeJump.x - viewportBeforeJump.width / 2) <= 1,
        "latest icon must be centered over the transcript, not the rail or Composer");
      const bottomGap = viewportBeforeJump.y + viewportBeforeJump.height - latestGeometry.y - latestGeometry.height;
      assert(bottomGap >= 8 && bottomGap <= 24, `unexpected latest icon bottom gap: ${bottomGap}`);
      const railTop = await turnList.evaluate((element) => element.scrollTop);
      await latest.click();
      await latest.waitFor({ state: "detached" });
      assert(await consoleScroll.evaluate((element) => element.scrollHeight - element.clientHeight - element.scrollTop <= 1));
      assertEquals(await consoleScroll.boundingBox(), viewportBeforeJump, "overlay must not resize the transcript");
      assertEquals(await turnList.evaluate((element) => element.scrollTop), railTop);
      await consoleScroll.evaluate((element) => { element.scrollTop = element.scrollHeight / 2; });
      await latest.waitFor();
      await latest.focus();
      await page.keyboard.press("Enter");
      await latest.waitFor({ state: "detached" });
      await consoleScroll.evaluate((element) => { element.scrollTop = element.scrollHeight / 2; });
      await latest.waitFor();
      await consoleScroll.evaluate((element) => { element.scrollTop = element.scrollHeight; });
      await latest.waitFor({ state: "detached" });

      const motion = await page.evaluate(() => (window as unknown as MotionWindow).latestMotion);
      assert(motion.some((animation) => animation.from === 0 && animation.to === 1 && animation.duration === 160), "latest icon must animate in");
      assert(motion.some((animation) => animation.from === 1 && animation.to === 0 && animation.duration === 160), "latest icon must animate out");
      await page.emulateMedia({ reducedMotion: "reduce" });
      await page.evaluate(() => { (window as unknown as MotionWindow).latestMotion = []; });
      await consoleScroll.evaluate((element) => { element.scrollTop = element.scrollHeight / 2; });
      await latest.waitFor();
      await latest.hover();
      const reducedStyle = await latest.evaluate((element) => {
        const style = getComputedStyle(element);
        return { translate: style.translate, duration: style.transitionDuration };
      });
      assertEquals(reducedStyle, { translate: "none", duration: "0s" });
      await latest.click();
      await latest.waitFor({ state: "detached" });
      assertEquals(await page.evaluate(() => (window as unknown as MotionWindow).latestMotion), [], "reduced motion must skip enter and exit animations");
      await page.emulateMedia({ reducedMotion: "no-preference" });

      const centering = await question12.evaluate((button) => {
        const turn = button.getBoundingClientRect();
        const bar = button.querySelector(".turn-bar")!.getBoundingClientRect();
        return Math.abs((turn.top + turn.bottom) / 2 - (bar.top + bar.bottom) / 2);
      });
      assert(centering <= 1, `turn marker is not vertically centered (${centering}px)`);
      assertEquals(await transcript.getByText("searched 1 time・ran 1 command", { exact: true }).count(), 12);
      await page.getByRole("button", { name: "Normal", exact: true }).click();
      const rowIds = await transcript.locator("[data-console-line-id]").evaluateAll((rows) =>
        rows.map((row) => row.getAttribute("data-console-line-id")));
      assertEquals(rowIds.length, 48);
      assertEquals(new Set(rowIds).size, rowIds.length);
      await page.getByRole("button", { name: "Overview", exact: true }).click();
      assertEquals(await transcript.getByText("searched 1 time・ran 1 command", { exact: true }).count(), 12);
      const sidebar = page.locator(".sidebar-frame");
      const settleSidebar = () => sidebar.evaluate(async (element) => {
        await Promise.all(element.getAnimations({ subtree: true }).map((animation) => animation.finished));
      });
      await checkSidebarModeSwitch(page, "Unpin sidebar");
      await page.mouse.move(1000, 30);
      await page.locator(".sidebar-frame.folded").waitFor();
      await settleSidebar();
      const mainBeforeHover = await page.locator(".app-shell__main").boundingBox();
      await checkSidebarSlide(page, false, true);
      await checkSidebarSlide(page, false, false);
      // The entire footer, including its button, is outside hover activation.
      const footer = sidebar.locator(".sidebar-control-row");
      await footer.hover({ position: { x: 4, y: 4 } });
      await page.waitForTimeout(250);
      assertEquals(await page.locator(".sidebar-frame.folded").count(), 1);
      await page.getByRole("button", { name: "Pin sidebar", exact: true }).hover();
      await page.waitForTimeout(250);
      assertEquals(await page.locator(".sidebar-frame.folded").count(), 1);
      await sidebar.locator(".sidebar-hover-region").hover({ position: { x: 10, y: 24 } });
      await page.locator(".sidebar-frame:not(.folded)").waitFor();
      await settleSidebar();
      await footer.hover({ position: { x: 4, y: 4 } });
      await page.locator(".sidebar-frame.folded").waitFor();
      await settleSidebar();
      // Use real mouse and keyboard events as well as deterministic motion sampling.
      await sidebar.hover({ position: { x: 10, y: 40 } });
      await page.locator(".sidebar-frame:not(.folded)").waitFor();
      await settleSidebar();
      assertEquals(await page.locator(".app-shell__main").boundingBox(), mainBeforeHover);
      const sidebarLink = sidebar.locator(".sidebar-frame-content a").first();
      await sidebarLink.focus();
      await page.mouse.move(1000, 30);
      await page.waitForTimeout(250);
      assertEquals(await page.locator(".sidebar-frame.folded").count(), 0, "keyboard focus must hold the preview open");
      await page.getByRole("link", { name: "Open Account", exact: true }).focus();
      await page.locator(".sidebar-frame.folded").waitFor();
      await settleSidebar();
      await sidebar.hover({ position: { x: 10, y: 40 } });
      await checkSidebarModeSwitch(page, "Pin sidebar");
      await page.mouse.move(1000, 30);
      await page.waitForTimeout(250);
      assertEquals(await page.locator(".sidebar-frame.folded").count(), 0);
      assertEquals(await page.evaluate(() => localStorage.getItem("yoi.sidebar.mode.v1")), "pinned");
      await checkSidebarModeSwitch(page, "Unpin sidebar");
      // Hold a preview open across reload; only the mode should be restored.
      await sidebarLink.focus();
      await page.mouse.move(1000, 30);
      assertEquals(await page.evaluate(() => localStorage.getItem("yoi.sidebar.mode.v1")), "hover");
      await page.reload();
      await page.getByRole("button", { name: "Pin sidebar", exact: true }).waitFor();
      assertEquals(await page.locator(".app-shell.sidebar-open").count(), 0);
      await page.setViewportSize({ width: 600, height: viewportHeight });
      await settleSidebar();
      await checkSidebarSlide(page, true, true);
      await checkSidebarSlide(page, true, false);
      await checkSidebarSlide(page, true, true);
      assertEquals(await page.evaluate(() => localStorage.getItem("yoi.sidebar.mode.v1")), "hover");
      await page.reload();
      await page.getByRole("button", { name: "Show sidebar", exact: true }).waitFor();
      assertEquals(await page.locator(".app-shell.sidebar-open").count(), 0);
      await page.getByRole("button", { name: "Show sidebar", exact: true }).click();
      await settleSidebar();
      await page.emulateMedia({ reducedMotion: "reduce" });
      await page.getByRole("button", { name: "Hide sidebar", exact: true }).click();
      const reducedSidebar = await page.locator(".sidebar-frame").evaluate((element) => ({
        animations: element.getAnimations({ subtree: true }).length,
        right: element.getBoundingClientRect().right,
        visibility: getComputedStyle(element).visibility,
      }));
      assertEquals(reducedSidebar, { animations: 0, right: 0, visibility: "hidden" });
      assertEquals(await page.locator(".app-shell__main").evaluate((element) => (element as HTMLElement).inert), false);
      if (viewportHeight === 600) {
        const touchContext = await browser.newContext({
          viewport: { width: 600, height: 600 }, hasTouch: true, isMobile: true,
          storageState: await context.storageState(),
        });
        const touchPage = await touchContext.newPage();
        await touchPage.goto(page.url());
        await touchPage.getByRole("button", { name: "Show sidebar", exact: true }).tap();
        await touchPage.getByRole("button", { name: "Pin sidebar", exact: true }).tap();
        assertEquals(await touchPage.evaluate(() => localStorage.getItem("yoi.sidebar.mode.v1")), "pinned");
        await touchPage.getByRole("button", { name: "Hide sidebar", exact: true }).tap();
        await touchPage.reload();
        await touchPage.getByRole("button", { name: "Show sidebar", exact: true }).waitFor();
        assertEquals(await touchPage.evaluate(() => localStorage.getItem("yoi.sidebar.mode.v1")), "pinned");
        // A wide touchscreen still has a temporary-open control in hover mode.
        await touchPage.setViewportSize({ width: 1024, height: 600 });
        await touchPage.getByRole("button", { name: "Unpin sidebar", exact: true }).tap();
        await touchPage.locator(".sidebar-frame.folded").waitFor();
        await touchPage.getByRole("button", { name: "Show sidebar", exact: true }).tap();
        await touchPage.locator(".sidebar-frame:not(.folded)").waitFor();
        await touchPage.getByRole("button", { name: "Hide sidebar", exact: true }).tap();
        await touchPage.locator(".sidebar-frame.folded").waitFor();
        assertEquals(await touchPage.evaluate(() => localStorage.getItem("yoi.sidebar.mode.v1")), "hover");
        await touchContext.close();
      }
      assertEquals(errors, []);
      await context.close();
    } finally {
      await browser.close();
    }
  } finally {
    server.kill("SIGTERM");
    await server.status;
  }
}

for (const viewportHeight of [600, 400, 340]) {
  Deno.test(`production Console keeps compact centered turns and pages history at ${viewportHeight}px`,
    () => checkConsoleHistory(viewportHeight));
}
