import { assertEquals } from "@std/assert";
import type { Page } from "playwright";

export async function freePort(): Promise<number> {
  const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
  const port = (listener.addr as Deno.NetAddr).port;
  listener.close();
  return port;
}

export async function waitForServer(url: string): Promise<void> {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    try {
      const response = await fetch(url);
      await response.body?.cancel();
      if (response.ok) return;
    } catch {
      // 所有するfixture serverの起動を有界に待つ。
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`fixture server did not become ready: ${url}`);
}

export async function mainScrollState(page: Page) {
  return await page.locator("main").evaluate((main) => ({
    scrollTop: main.scrollTop,
    scrollHeight: main.scrollHeight,
    clientHeight: main.clientHeight,
  }));
}

export async function assertNoPageWideOverflow(page: Page): Promise<void> {
  assertEquals(
    await page.evaluate(() => {
      const document = (globalThis as any).document;
      return document.documentElement.scrollWidth ===
        document.documentElement.clientWidth;
    }),
    true,
  );
}

export async function assertMainOwnsVerticalScroll(page: Page): Promise<void> {
  const nested = await page.locator("main").evaluate((main) =>
    [...main.querySelectorAll("*")]
      .filter((element) => {
        const style = (globalThis as any).getComputedStyle(element);
        return ["auto", "scroll"].includes(style.overflowY) &&
          element.scrollHeight > element.clientHeight + 1;
      })
      .map((element) => `${element.tagName.toLowerCase()}.${element.className}`)
  );
  assertEquals(nested, []);
}
