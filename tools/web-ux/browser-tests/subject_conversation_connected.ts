// Explicit connected test driver. Rust owns ephemeral Backend/Runtime/SQLite;
// this browser runs the production Web build. Only the external model and
// deliberate lost transport acknowledgement are substituted. No live instance.
import { assert, assertEquals } from "@std/assert";
import { chromium } from "playwright";
import {
  subjectChatHref,
  subjectHref,
} from "../../../web/workspace/src/lib/workspace/subjektiv/routes.ts";

const input = JSON.parse(await new Response(Deno.stdin.readable).text());
const origin = new URL(input.origin);
assert(origin.hostname === "127.0.0.1" && origin.protocol === "http:");
const base = `/api/w/${encodeURIComponent(input.workspace_id)}/chat/conversations`;
const detail = subjectHref(input.workspace_id, input.subject_id);
const chat = subjectChatHref(input.workspace_id, input.subject_id);
const legacy = `/w/${encodeURIComponent(input.workspace_id)}/memory/${
  encodeURIComponent(input.subject_id)
}`;
const suffix = "?from=legacy&encoded=%2Fkeep%20exact#subject-heading";
const browser = await chromium.launch({ headless: true });
try {
  const context = await browser.newContext({
    extraHTTPHeaders: { Authorization: `Bearer ${input.owner_token}` },
  });
  const page = await context.newPage();
  page.setDefaultTimeout(30000);
  const posts: Record<string, unknown>[] = [];
  const pageErrors: string[] = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  page.on("request", (request) => {
    if (
      request.method() === "POST" &&
      new URL(request.url()).pathname ===
        `${base}/${encodeURIComponent(input.conversation_id)}/messages`
    ) posts.push(request.postDataJSON());
  });
  const response = await page.goto(`${origin.origin}${legacy}${suffix}`);
  assertEquals(response?.status(), 200);
  await page.waitForURL(`${origin.origin}${detail}${suffix}`);
  const open = page.getByRole("link", { name: "Open conversation" });
  assertEquals(await open.getAttribute("href"), chat);
  await open.click();
  await page.waitForURL(`${origin.origin}${chat}`);
  await page.getByText("No public messages yet.").waitFor();
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
    const response = await page.goto(`${origin.origin}${entry}${suffix}`);
    assertEquals(response?.status(), 200);
    await page.waitForURL(`${origin.origin}${chat}${suffix}`);
    await page.getByText("public reply 3", { exact: true }).waitFor();
    assertEquals(await history.getByRole("listitem").count(), 4);
  }
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
  assertEquals(await page.getByRole("textbox").count(), 0);
  assertEquals(await history.getByRole("listitem").count(), 0);
  assertEquals(pageErrors, []);
  console.log(
    "real Web -> Backend -> Runtime -> explicit public replies passed; scripted model, API-token browser transport; Body route not yet integrated",
  );
} finally {
  await browser.close();
}
