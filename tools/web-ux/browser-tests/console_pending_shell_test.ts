/// <reference lib="dom" />
import { assert, assertEquals } from "@std/assert";
import { dirname, fromFileUrl, join } from "@std/path";
import { chromium } from "playwright";

const root = join(dirname(fromFileUrl(import.meta.url)), "../../..");
Deno.test("Console pending scopes are independent and match Tasks row typography", async () => {
  const build = await new Deno.Command(Deno.execPath(), {
    args: ["task", "build"],
    cwd: join(root, "web/workspace"),
    stdout: "piped",
    stderr: "piped",
  }).output();
  assert(build.success, new TextDecoder().decode(build.stderr));
  const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
  const port = (listener.addr as Deno.NetAddr).port;
  listener.close();
  const url = `http://127.0.0.1:${port}`;
  const server = new Deno.Command(Deno.execPath(), {
    args: [
      "run",
      "--allow-net",
      "--allow-read",
      join(root, "tools/web-ux/browser-tests/console_history_fixture_server.ts"),
      String(port),
      join(root, "web/workspace/build"),
    ],
    stdout: "null",
    stderr: "null",
  }).spawn();
  try {
    let started = false;
    for (let attempt = 0; attempt < 100; attempt++) {
      try {
        const response = await fetch(`${url}/health`);
        await response.body?.cancel();
        if (response.ok) {
          started = true;
          break;
        }
      } catch { /* owned fixture is starting */ }
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    assert(started);
    const browser = await chromium.launch({ headless: true });
    try {
      for (const colorScheme of ["light", "dark"] as const) {
        for (const width of [1440, 768, 390, 320]) {
          const context = await browser.newContext({
            viewport: { width, height: 900 },
            colorScheme,
            reducedMotion: "reduce",
          });
          try {
            const page = await context.newPage();
            const errors: string[] = [];
            page.on("pageerror", (error) => errors.push(String(error)));
            page.on("console", (message) => {
              if (message.type() === "error") errors.push(message.text());
            });
            page.on("requestfailed", (request) => errors.push(request.url()));
            for (const mode of ["both", "queue", "notifications", "legacy", "empty"]) {
              await page.goto(
                `${url}/w/console-history-review/workers/W-900-console-fixture/console?pending=${mode}`,
              );
              await page.locator(".task-mini-row").waitFor();
              const queue = mode === "both" || mode === "queue";
              const notifications = ["both", "notifications", "legacy"].includes(mode);
              assertEquals(
                await page.getByRole("group", { name: "Queued inputs", exact: true }).count(),
                queue ? 1 : 0,
              );
              assertEquals(
                await page.getByRole("group", { name: "Notifications", exact: true }).count(),
                notifications ? 1 : 0,
              );
              assertEquals(await page.getByRole("heading", { name: "0 Queued" }).count(), 0);
              const region = page.getByRole("region", { name: "Pending activations" });
              assertEquals(await region.count(), mode === "empty" ? 0 : 1);
              if (mode === "empty") continue;
              const geometry = await region.evaluate((region) => {
                const task = document.querySelector<HTMLElement>(".task-mini-row")!;
                const taskStyle = getComputedStyle(task);
                const metrics = (node: Element) => {
                  const style = getComputedStyle(node);
                  return {
                    height: node.getBoundingClientRect().height,
                    family: style.fontFamily,
                    size: style.fontSize,
                    line: style.lineHeight,
                  };
                };
                return {
                  task: metrics(task),
                  rows: [...region.querySelectorAll("h2, li")].map(metrics),
                  lists: [...region.querySelectorAll("ol")].map((list) => ({
                    margin: getComputedStyle(list).margin,
                    padding: getComputedStyle(list).padding,
                    gap: getComputedStyle(list).rowGap,
                  })),
                  columns: [...region.querySelectorAll(".pending-column")].map((column) =>
                    column.getBoundingClientRect().toJSON()
                  ),
                  width: region.getBoundingClientRect().width,
                  pageOverflow: document.documentElement.scrollWidth > innerWidth,
                  mainOverflow: document.querySelector("main")!.scrollWidth >
                    document.querySelector("main")!.clientWidth,
                  previews: [...region.querySelectorAll(".pending-submission-preview")].map((
                    preview,
                  ) => ({
                    title: preview.getAttribute("title"),
                    whitespace: getComputedStyle(preview).whiteSpace,
                    overflow: getComputedStyle(preview).textOverflow,
                  })),
                  expectedGap: getComputedStyle(task.closest(".task-mini")!).rowGap,
                  taskMono: taskStyle.fontFamily.includes("mono"),
                };
              });
              assertEquals(geometry.task.height, 16);
              assertEquals(geometry.taskMono, true);
              for (const row of geometry.rows) {
                assertEquals(
                  row,
                  geometry.task,
                  `${width}px/${mode}: heading and preview rows must match Tasks`,
                );
              }
              for (const list of geometry.lists) {
                assertEquals(list, { margin: "0px", padding: "0px", gap: geometry.expectedGap });
              }
              if (mode === "both") assertEquals(geometry.columns[0].y, geometry.columns[1].y);
              else {assertEquals(
                  geometry.columns[0].width,
                  geometry.width,
                  "a lone scope must not reserve an empty column",
                );}
              assertEquals(geometry.pageOverflow, false);
              assertEquals(geometry.mainOverflow, false);
              for (const preview of geometry.previews) {
                assert(preview.title);
                assertEquals(preview.whitespace, "nowrap");
                assertEquals(preview.overflow, "ellipsis");
              }
              if (queue) {
                const cancel = page.getByRole("button", { name: "Cancel queued input 1" });
                assertEquals((await cancel.boundingBox())!.height, 16);
                assertEquals(
                  await cancel.isDisabled(),
                  true,
                  "retained sessions cannot mutate the queue",
                );
                await page.locator(".pending-queue li").first().hover();
                assertEquals(
                  await cancel.evaluate((button) => getComputedStyle(button).opacity),
                  "1",
                );
              }
              if (mode === "legacy") {
                await page.getByText("2 pending · Preview unavailable", { exact: true }).waitFor();
              }
            }
            assertEquals(errors, []);
          } finally {
            await context.close();
          }
        }
      }
    } finally {
      await browser.close();
    }
  } finally {
    try {
      server.kill("SIGTERM");
    } catch (error) {
      if (!(error instanceof Deno.errors.NotFound)) throw error;
    }
    await server.status;
  }
});
