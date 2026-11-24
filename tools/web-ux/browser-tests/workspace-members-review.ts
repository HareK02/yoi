// 実際のページと合成HTTP境界を使用する。認可の保証はBackend結合テストが担当する。
import { chromium } from "playwright";
import { resolve } from "@std/path";
import { assertEquals } from "@std/assert";
import { startOwnedProcesses, stopOwnedProcesses } from "../src/processes.ts";
const root = resolve(import.meta.dirname!, "../../..");
const phase = Deno.args[0] ?? "before";
const build = Deno.args[1] ?? "web/workspace/build";
const output = resolve(root, Deno.args[3] ?? "target/web-ux/workspace-members", phase);
const base = "http://127.0.0.1:15484";
const processes = await startOwnedProcesses([{
  id: "workspace-members", command: "deno", cwd: root,
  args: ["run", "--config", "tools/web-ux/deno.json", "--allow-read", "--allow-net", "tools/web-ux/browser-tests/workspace_home_fixture_server.ts", "15484", build],
  readyUrl: `${base}/health`,
}], `${root}/tools/web-ux/scenarios/anonymous-entry.json`, `${output}/processes`, []);
let browser: Awaited<ReturnType<typeof chromium.launch>> | undefined;
try {
  browser = await chromium.launch(Deno.args[2] ? { executablePath: Deno.args[2] } : {});
  for (const theme of ["light", "dark"] as const) for (const width of [1440, 768, 390]) {
    const context = await browser.newContext({ viewport: { width, height: 1000 }, colorScheme: theme, reducedMotion: "reduce" });
    const page = await context.newPage();
    const errors: string[] = [];
    const expectedErrors: string[] = [];
    const requests: string[] = [];
    let denied = false;
    page.on("pageerror", e => errors.push(e.message));
    page.on("console", m => {
      if (m.type() === "error") (denied && m.text().includes("403") ? expectedErrors : errors).push(m.text());
    });
    page.on("requestfailed", r => { if (r.failure()?.errorText !== "net::ERR_ABORTED") errors.push(`${new URL(r.url()).pathname}: ${r.failure()?.errorText}`); });
    const owner = { account_id: "owner", handle: "owner", display_name: "Workspace owner", is_owner: true, membership_key: null };
    const alice = { account_id: "alice", handle: "alice", display_name: width === 390 ? "長い表示名 — Distributed development and documentation team" : "Alice Example", is_owner: false, membership_key: "member-alice" };
    let members = [owner, alice];
    let changeKey = "original";
    if (phase === "after") await page.route("**/api/w/*/members**", async route => {
      const request = route.request();
      requests.push(`${request.method()} ${new URL(request.url()).pathname}`);
      if (denied) { await route.fulfill({ status: 403, json: { error: "Forbidden", message: "Workspace owner required" } }); return; }
      if (request.method() === "GET") { await route.fulfill({ json: { workspace_id: "home-owner", membership_change_key: changeKey, members, next_after_account_id: null } }); return; }
      const body = request.postDataJSON();
      assertEquals(typeof body.operation_key, "string");
      const add = request.method() === "POST";
      if (add) {
        assertEquals(body.account_handle, "bob");
        assertEquals(body.expected_membership_change_key, changeKey);
        members.push({ ...alice, account_id: "bob", handle: "bob", display_name: "Bob Example", membership_key: "member-bob" });
      } else {
        assertEquals(body.expected_membership_key, "member-alice");
        members = members.filter(m => m.account_id !== "alice");
      }
      changeKey = add ? "added" : "removed";
      await route.fulfill({ json: { workspace_id: "home-owner", account_id: add ? "bob" : "alice", membership_key: add ? "member-bob" : null, membership_change_key: changeKey, present: add, replayed: false } });
    });
    const response = await page.goto(`${base}/w/home-owner/settings${phase === "after" ? "/members" : ""}`);
    await page.getByRole("heading", { name: phase === "after" ? "Members" : "Workspace Identity", exact: true }).waitFor();
    if (phase === "after") await page.getByText(alice.display_name, { exact: true }).waitFor();
    async function capture(name: string) {
      await page.evaluate(() => document.fonts.ready);
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth > innerWidth);
      const directory = `${output}/${theme}-${width}`;
      await Deno.mkdir(directory, { recursive: true });
      const image = `${directory}/${name}.png`;
      await page.screenshot({ path: image, fullPage: true });
      const hash = Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", await Deno.readFile(image)))).map(x=>x.toString(16).padStart(2,"0")).join("");
      await Deno.writeTextFile(`${directory}/${name}-review-context.json`, JSON.stringify({ phase, theme, width, status: response?.status(), errors, expectedErrors, requests, overflow, image, sha256: hash, accessibility: await page.locator("body").ariaSnapshot() }, null, 2));
      assertEquals(response?.status(), 200); assertEquals(errors, []); assertEquals(overflow, false);
    }
    await capture("settings");
    if (phase === "after") {
      await page.getByRole("button", { name: "Add member", exact: true }).click();
      const input = page.getByLabel("Account ID (handle)");
      await input.fill("bob");
      assertEquals(await input.evaluate(e => e === document.activeElement), true);
      await capture("add");
      await page.getByRole("button", { name: "Add member", exact: true }).click();
      await page.getByText("Bob Example", { exact: true }).waitFor();
      await page.getByRole("button", { name: "Remove @alice", exact: true }).click();
      await page.getByText(alice.display_name, { exact: true }).waitFor({ state: "detached" });
      await page.getByText("Bob Example", { exact: true }).waitFor();
      await capture("changed");
      denied = true;
      await page.getByRole("button", { name: "Reload members", exact: true }).click();
      await page.getByRole("alert").waitFor();
      assertEquals(await page.getByRole("button", { name: "Add member", exact: true }).count(), 0);
      assertEquals(await page.getByText("Bob Example", { exact: true }).count(), 0);
      await capture("denied");
    }
    await context.close();
  }
} finally { await browser?.close(); await stopOwnedProcesses(processes); }
