// 明示選択の接続driver。隔離Backend/Runtime/SQLiteはRust fixtureが所有し、
// 本番Webから共有Bodyと会話APIを操作する。外部model・ACK喪失・Busy時刻のみ制御する。
import { assert, assertEquals } from "@std/assert";
import { chromium, type Page } from "playwright";
import { sha256File } from "../src/artifacts.ts";
import { assertNoPageWideOverflow } from "./memory_document_harness.ts";
import {
  subjectChatHref,
  subjectHref,
} from "../../../web/workspace/src/lib/workspace/subjektiv/routes.ts";

const input = JSON.parse(await new Response(Deno.stdin.readable).text());
const origin = new URL(input.origin);
assert(origin.hostname === "127.0.0.1" && origin.protocol === "http:");
const base = `/api/w/${encodeURIComponent(input.workspace_id)}/chat/conversations`;
const bodyPath = `/api/w/${encodeURIComponent(input.workspace_id)}/subjektiv/subjects/${
  encodeURIComponent(input.subject_id)
}/body`;
const detail = subjectHref(input.workspace_id, input.subject_id);
const chat = subjectChatHref(input.workspace_id, input.subject_id);
const legacy = `/w/${encodeURIComponent(input.workspace_id)}/memory/${
  encodeURIComponent(input.subject_id)
}`;
const suffix = "?from=legacy&encoded=%2Fkeep%20exact#subject-heading";
const output = `${input.artifact_dir}/run-${Date.now()}`;
await Deno.mkdir(output, { recursive: true });
const git = async (args: string[]) => {
  const result = await new Deno.Command("git", { args, stdout: "piped" }).output();
  assert(result.success);
  return new TextDecoder().decode(result.stdout).trim();
};
const pageErrors: string[] = [];
const captures: unknown[] = [];
const manifest = {
  evidence: "actual-Web-Backend-SQLite-signed-Runtime; external model scripted",
  source: {
    commit: await git(["rev-parse", "HEAD"]),
    status: await git(["status", "--porcelain"]),
  },
  driverSha256: await sha256File(new URL(import.meta.url).pathname),
  buildIndexSha256: await sha256File("web/workspace/build/index.html"),
  visualVerdict: "UNREVIEWED: implementation author must view images",
  status: "running",
  pageErrors,
  captures,
};
const save = () =>
  Deno.writeTextFile(`${output}/review-context.json`, JSON.stringify(manifest, null, 2));
async function capture(page: Page, id: string, theme: "light" | "dark" = "light") {
  await assertNoPageWideOverflow(page);
  const image = `${output}/${id}.png`;
  await page.screenshot({ path: image, fullPage: true, animations: "disabled" });
  const accessibility = `${output}/${id}.aria.txt`;
  await Deno.writeTextFile(accessibility, await page.locator("main").ariaSnapshot());
  captures.push({
    id,
    route: new URL(page.url()).pathname,
    viewport: page.viewportSize(),
    theme,
    image,
    imageSha256: await sha256File(image),
    accessibility,
    visibleAlerts: await page.getByRole("alert").allTextContents(),
    mainWidth: await page.locator("main").evaluate((el) => el.clientWidth),
  });
  await save();
}
const browser = await chromium.launch({ headless: true });
let activePage: Page | null = null;
try {
  const context = await browser.newContext({
    viewport: { width: 1440, height: 1000 },
    colorScheme: "light",
    extraHTTPHeaders: { Authorization: `Bearer ${input.owner_token}` },
  });
  const page = await context.newPage();
  activePage = page;
  page.setDefaultTimeout(30000);
  const posts: Record<string, unknown>[] = [];
  const bodyPosts: { path: string; input: Record<string, unknown> }[] = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  page.on("request", (request) => {
    const path = new URL(request.url()).pathname;
    if (request.method() !== "POST") return;
    if (path === `${base}/${encodeURIComponent(input.conversation_id)}/messages`) {
      posts.push(request.postDataJSON());
    }
    if (path.startsWith(`${bodyPath}/`)) bodyPosts.push({ path, input: request.postDataJSON() });
  });
  // 有限の状態観測で同期する。refreshやACKからRuntimeのidleを推論しない。
  async function idle() {
    await page.waitForFunction(async (path) => {
      const response = await fetch(path, { cache: "no-store" });
      if (!response.ok) return false;
      const body = await response.json();
      return body.observation.kind === "available" &&
        body.observation.worker.worker_state?.state.kind === "idle";
    }, bodyPath);
    await page.getByRole("button", { name: "Refresh body state", exact: true }).click();
    await page.getByText("idle", { exact: true }).first().waitFor();
  }
  const response = await page.goto(`${origin.origin}${legacy}${suffix}`);
  assertEquals(response?.status(), 200);
  await page.waitForURL(`${origin.origin}${detail}${suffix}`);
  await page.getByText("No current conversation body.", { exact: true }).waitFor();
  await capture(page, "light-1440-absent-detail");
  await page.getByRole("button", { name: "Start conversation body", exact: true }).click();
  await idle();
  assertEquals(bodyPosts.length, 1);
  assertEquals(bodyPosts[0].path, `${bodyPath}/start`);
  assertEquals(bodyPosts[0].input.expected_current, null);
  const initialBody = await (await context.request.get(`${origin.origin}${bodyPath}`)).json();
  const open = page.getByRole("link", { name: "Open conversation" });
  assertEquals(await open.getAttribute("href"), chat);
  await open.click();
  await page.waitForURL(`${origin.origin}${chat}`);
  await page.getByText("No public messages yet.").waitFor();
  await capture(page, "light-1440-empty-chat");
  for (const alias of ["conversation", "chat"]) {
    const response = await page.goto(`${origin.origin}${legacy}/${alias}${suffix}`);
    assertEquals(response?.status(), 200);
    await page.waitForURL(`${origin.origin}${chat}${suffix}`);
    await page.getByText("No public messages yet.").waitFor();
    assertEquals(
      await page.getByRole("link", { name: "← Subject details" }).getAttribute("href"),
      detail,
    );
  }
  const direct = await context.newPage();
  try {
    const response = await direct.goto(`${origin.origin}${chat}?from=direct#saved-messages`);
    assertEquals(response?.status(), 200);
    await direct.getByText("No public messages yet.").waitFor();
    await direct.reload();
    await direct.getByText("No public messages yet.").waitFor();
    assertEquals(
      new URL(direct.url()).pathname + new URL(direct.url()).search + new URL(direct.url()).hash,
      `${chat}?from=direct#saved-messages`,
    );
  } finally {
    await direct.close();
  }
  assertEquals(posts.length, 0, "route redirects and direct loads must not submit input");
  const composer = page.getByRole("textbox", { name: `Message to ${input.subject_name}` });
  await composer.fill("WEB_CONNECTED_FIRST_INPUT");
  await page.getByRole("button", { name: "Send message", exact: true }).click();
  await page.getByText("Message saved; Runtime acceptance is unknown. Retry only this same input.")
    .waitFor();
  await page.getByRole("button", { name: "Retry same input" }).click();
  await page.getByText("Runtime accepted this input. Processing and replies are not confirmed.")
    .waitFor();
  assertEquals(posts.length, 2);
  assertEquals(posts[0], posts[1]);
  let lost = false;
  await page.route(`**${base}/**/messages`, async (route) => {
    if (!lost && route.request().method() === "POST") {
      lost = true;
      const actual = await route.fetch();
      assertEquals(actual.status(), 200);
      await route.abort("failed");
    } else await route.continue();
  });
  await composer.fill("WEB_CONNECTED_BUSY_INPUT");
  await page.getByRole("button", { name: "Send message", exact: true }).click();
  await page.getByRole("alert").filter({ hasText: /outcome could not be confirmed/ }).waitFor();
  await composer.fill("edited next draft must not replace retained input");
  assert(await page.getByRole("button", { name: "Send message", exact: true }).isDisabled());
  const retry = posts[2];
  const conflict = await context.request.post(
    `${origin.origin}${base}/${encodeURIComponent(input.conversation_id)}/messages`,
    { data: { ...retry, text: "changed payload under original operation" } },
  );
  assertEquals(conflict.status(), 409);
  await page.goto(`${origin.origin}${legacy}/conversation${suffix}`);
  await page.waitForURL(`${origin.origin}${chat}${suffix}`);
  await page.getByText("Previous send outcome is unknown. Its original input is retained.")
    .waitFor();
  await page.reload();
  await page.getByText("Previous send outcome is unknown. Its original input is retained.")
    .waitFor();
  await page.getByRole("button", { name: "Retry same input" }).click();
  await page.getByText("Runtime accepted this input. Processing and replies are not confirmed.")
    .waitFor();
  assertEquals(posts.length, 4);
  assertEquals(posts[2], posts[3]);
  await page.getByRole("button", { name: "Refresh history", exact: true }).click();
  await page.getByText("public reply 2", { exact: true }).waitFor();
  await page.getByText("public reply 3", { exact: true }).waitFor();
  const history = page.getByRole("list", { name: "Saved public messages" });
  assertEquals(await history.getByRole("listitem").count(), 4);
  assertEquals(
    await page.getByText("INTERNAL_ASSISTANT_TEXT_NOT_A_PUBLIC_CHAT_MESSAGE").count(),
    0,
  );
  assertEquals(await history.getByText("WEB_CONNECTED_BUSY_INPUT", { exact: true }).count(), 1);
  for (const entry of [chat, `${legacy}/conversation`, `${legacy}/chat`]) {
    const target = `${origin.origin}${entry}${suffix}`;
    const response = page.url() === target ? await page.reload() : await page.goto(target);
    assertEquals(response?.status(), 200);
    await page.waitForURL(`${origin.origin}${chat}${suffix}`);
    await page.getByText("public reply 3", { exact: true }).waitFor();
    assertEquals(await history.getByRole("listitem").count(), 4);
  }
  await idle();
  // 共有Bodyの実停止後にACKだけ失う。reload後も完全なstop intentを維持し、
  // read-only outcome照会は追加stopを送らないことを監視する。
  let stopLost = false;
  await page.route(`**${bodyPath}/stop`, async (route) => {
    if (!stopLost) {
      stopLost = true;
      const actual = await route.fetch();
      assertEquals(actual.status(), 200);
      await route.abort("failed");
    } else await route.continue();
  });
  await page.getByRole("button", { name: "Stop conversation body", exact: true }).click();
  await page.getByRole("alert").filter({ hasText: /Request outcome unknown/ }).waitFor();
  assertEquals(bodyPosts.length, 2);
  const stopRequest = bodyPosts[1].input;
  assertEquals(stopRequest.expected_current, initialBody.current);
  await page.reload();
  await page.getByRole("button", { name: "Check stop outcome", exact: true }).click();
  await page.getByText("Historical stop confirmed.", { exact: true }).waitFor();
  await page.getByText("Body stopped.", { exact: true }).waitFor();
  assertEquals(bodyPosts.length, 3);
  assertEquals(bodyPosts[2], { path: `${bodyPath}/stop/outcome`, input: stopRequest });
  assertEquals(await history.getByRole("listitem").count(), 4);
  await capture(page, "light-1440-stopped-retained-chat");
  await page.getByRole("button", { name: "Restart conversation body", exact: true }).click();
  await idle();
  const restored = await (await context.request.get(`${origin.origin}${bodyPath}`)).json();
  assertEquals(
    restored.current,
    initialBody.current,
    "restore must keep the singleton Body, not create another",
  );
  await composer.fill("WEB_CONNECTED_AFTER_RESTART");
  await page.getByRole("button", { name: "Send message", exact: true }).click();
  await page.getByText("Runtime accepted this input. Processing and replies are not confirmed.")
    .waitFor();
  // ACKや初期idleは処理済みの証拠ではない。scripted modelの受信観測後に
  // 本番Runtimeのidleを待ち、この返信なしturnを停止で潰さない。
  const observed = await context.request.get(`${origin.origin}/__fixture/model-resumed`);
  assertEquals(observed.status(), 200);
  assertEquals(await observed.json(), { observed: true });
  await idle();
  await page.getByRole("button", { name: "Refresh history", exact: true }).click();
  await history.getByText("WEB_CONNECTED_AFTER_RESTART", { exact: true }).waitFor();
  assertEquals(
    await history.getByRole("listitem").count(),
    5,
    "no reply turn must not expose internal assistant text",
  );
  for (const width of [1440, 768, 390]) {
    for (const theme of ["light", "dark"] as const) {
      await page.setViewportSize({ width, height: 1000 });
      await page.emulateMedia({ colorScheme: theme });
      await capture(page, `${theme}-${width}-restored-chat`, theme);
    }
  }
  await page.getByRole("button", { name: "Stop conversation body", exact: true }).click();
  const stopDisposition = await Promise.race([
    page.getByText("Body stopped.", { exact: true }).waitFor().then(() => "stopped"),
    page.getByText("Request rejected.", { exact: true }).waitFor().then(() => "rejected"),
  ]);
  if (stopDisposition === "rejected") {
    // 実Backendの既知の未実行拒否だけが新観測・新依頼へ進める。
    // unknownをここで清算したり、同IDの入力を差し替えたりしない。
    await page.getByRole("button", { name: "Refresh body state", exact: true }).click();
    await page.getByRole("button", { name: "Stop conversation body", exact: true }).click();
    await page.getByText("Body stopped.", { exact: true }).waitFor();
    const stops = bodyPosts.filter((post) => post.path === `${bodyPath}/stop`);
    assert(stops.at(-1)!.input.request_id !== stops.at(-2)!.input.request_id);
  }
  assertEquals(await history.getByRole("listitem").count(), 5);
  assertEquals(bodyPosts.filter((post) => post.path === `${bodyPath}/start`).length, 2);
  assertEquals(
    bodyPosts.filter((post) => post.path === `${bodyPath}/stop`).length,
    stopDisposition === "rejected" ? 3 : 2,
  );
  const deniedPath = `${origin.origin}${base}/${
    encodeURIComponent(input.conversation_id)
  }/messages`;
  await context.setExtraHTTPHeaders({ Authorization: `Bearer ${input.foreign_token}` });
  const denied = await context.request.post(deniedPath, {
    data: { operation_id: "foreign-cannot-post", text: "must not append" },
  });
  assertEquals(denied.status(), 403);
  await page.getByRole("button", { name: "Refresh history", exact: true }).click();
  await page.getByRole("alert").filter({ hasText: /do not have access/ }).waitFor();
  await page.getByRole("button", { name: "Refresh body state", exact: true }).click();
  await page.getByRole("alert").filter({ hasText: /Body state could not be confirmed/ }).waitFor();
  assertEquals(await page.getByRole("textbox").count(), 0);
  assertEquals(await history.getByRole("listitem").count(), 0);
  await capture(page, "dark-390-permission-denied", "dark");
  assertEquals(pageErrors, []);
  manifest.status = "passed";
  console.log(
    "real Web -> Backend -> Runtime -> explicit public replies passed; scripted model; shared Body start/stop/restore and canonical/legacy routes included",
  );
} catch (cause) {
  manifest.status = "failed";
  if (activePage) {
    console.error("Visible alerts:", await activePage.getByRole("alert").allTextContents());
    await capture(activePage, "failure").catch(() => {});
  }
  throw cause;
} finally {
  await save();
  await browser.close();
}
