#!/usr/bin/env -S deno run -A
/** 実Webの会話画面を合成HTTP/WebSocketで撮影する。実Backend/Runtimeの証拠ではない。 */
import { assert, assertEquals } from "@std/assert";
import { dirname, fromFileUrl, join, resolve } from "@std/path";
import { type Browser, chromium, type Page, type Request } from "playwright";
import type {
  ChatMessage,
  ChatUserMessageRequest,
  MessagingCommand,
  MessagingResult,
} from "../../../web/workspace/src/lib/generated/chat-api.ts";
import { sha256File } from "../src/artifacts.ts";
import { startOwnedProcesses, stopOwnedProcesses } from "../src/processes.ts";
import { assertNoPageWideOverflow, freePort } from "./memory_document_harness.ts";

const root = resolve(dirname(fromFileUrl(import.meta.url)), "../../..");
const [buildArg, runId, mode] = Deno.args;
if (
  !buildArg || !runId || !/^[a-zA-Z0-9._-]+$/.test(runId) || !["before", "after"].includes(mode)
) {
  throw new Error(
    "usage: subject_conversation_review.ts <actual-build-path> <unique-run-id> <before|after>",
  );
}
const buildRoot = resolve(buildArg);
await Deno.stat(join(buildRoot, "index.html"));
const output = join(root, "target/web-ux/t808", runId);
await Deno.mkdir(dirname(output), { recursive: true });
await Deno.mkdir(output);
const port = await freePort();
const base = `http://127.0.0.1:${port}`;
const subject = "empty-subject";
const subjectRoute = `/w/memory-review/${mode === "before" ? "memory" : "subjektiv"}/${subject}`;
const captures: unknown[] = [];
const failures: string[] = [];
const scenarios: unknown[] = [];
const faults: { kind: string; message: string; expected: boolean }[] = [];
const git = async (args: string[]) => {
  const result = await new Deno.Command("git", { args, cwd: root, stdout: "piped" }).output();
  return result.success ? new TextDecoder().decode(result.stdout).trim() : null;
};
const manifest = {
  evidence: "actual-Web-with-synthetic-HTTP-and-WebSocket-NOT-real-Backend-or-Runtime",
  mode,
  buildRoot,
  buildIndexSha256: await sha256File(join(buildRoot, "index.html")),
  source: {
    commit: await git(["rev-parse", "HEAD"]),
    status: await git(["status", "--porcelain"]),
    runnerSha256: await sha256File(fromFileUrl(import.meta.url)),
  },
  browser: { name: "chromium", version: "" },
  visualVerdict: "UNREVIEWED — implementation author must ViewImage and inspect this context",
  status: "running",
  captures,
  scenarios,
  faults,
  failures,
};
const save = () =>
  Deno.writeTextFile(join(output, "review-context.json"), JSON.stringify(manifest, null, 2));
async function capture(page: Page, id: string) {
  const image = join(output, `${id}.png`);
  const accessibility = join(output, `${id}.aria.txt`);
  await page.screenshot({ path: image });
  await Deno.writeTextFile(accessibility, await page.locator("body").ariaSnapshot());
  captures.push({
    id,
    route: new URL(page.url()).pathname,
    viewport: page.viewportSize(),
    image,
    imageSha256: await sha256File(image),
    accessibility,
    accessibilitySha256: await sha256File(accessibility),
    visibleAlerts: await page.getByRole("alert").allTextContents(),
    focus: await page.locator(":focus").evaluateAll((els) =>
      els.map((el) => ({ tag: el.tagName, id: el.id }))
    ),
    mainWidth: await page.locator("main").evaluate((el) => el.clientWidth),
    reviewed: false,
  });
  await assertNoPageWideOverflow(page);
  await save();
}
const owned = await startOwnedProcesses(
  [{
    id: "conversation-fixture",
    command: Deno.execPath(),
    cwd: join(root, "tools/web-ux"),
    args: [
      "run",
      "--cached-only",
      "-A",
      join(root, "tools/web-ux/browser-tests/memory_document_fixture_server.ts"),
      String(port),
      buildRoot,
      "3",
    ],
    readyUrl: `${base}/health`,
  }],
  join(output, "scenario.json"),
  join(output, "logs"),
  [],
);
let browser: Browser | undefined;
try {
  browser = await chromium.launch({ headless: true });
  manifest.browser.version = browser.version();
  for (const theme of ["light", "dark"] as const) {
    for (const width of [1440, 768, 390]) {
      const prefix = `${theme}-${width}`;
      const context = await browser.newContext({
        viewport: { width, height: 900 },
        colorScheme: theme,
        reducedMotion: "reduce",
      });
      const page = await context.newPage();
      page.setDefaultTimeout(15000);
      const navigationRequests = new WeakSet<Request>();
      const pendingRequests = new Set<Request>();
      let leaving = false;
      let httpChatRequests = 0;
      context.on("request", (request) => {
        if (request.url().includes("/chat/conversations")) httpChatRequests++;
      });
      function beginNavigation() {
        leaving = true;
        for (const request of pendingRequests) navigationRequests.add(request);
      }
      page.on("request", (request) => {
        pendingRequests.add(request);
        if (leaving) navigationRequests.add(request);
      });
      page.on("requestfinished", (request) => pendingRequests.delete(request));
      page.on("requestfailed", (request) => pendingRequests.delete(request));
      let faultMode: "normal" | "ack-loss" | "forbidden" = "normal";
      page.on(
        "pageerror",
        (e) => faults.push({ kind: "pageerror", message: e.message, expected: false }),
      );
      page.on("console", (e) => {
        if (["warning", "error"].includes(e.type())) {
          faults.push({
            kind: `console-${e.type()}`,
            message: e.text(),
            expected: false,
          });
        }
      });
      page.on("requestfailed", (r) =>
        faults.push({
          kind: "requestfailed",
          message: `${r.method()} ${r.url()} ${r.failure()?.errorText}`,
          expected: navigationRequests.has(r) && r.method() === "GET" &&
            r.failure()?.errorText === "net::ERR_ABORTED",
        }));
      page.on("response", (r) => {
        if (r.status() >= 400) {
          faults.push({
            kind: "http",
            message: `${r.status()} ${r.url()}`,
            expected: false,
          });
        }
      });
      try {
        const messages: ChatMessage[] = [];
        const requests: ChatUserMessageRequest[] = [];
        const commands: MessagingCommand[] = [];
        let droppedSavedAck = false;
        let rejectedHistory = false;
        const scenario = {
          prefix,
          messages,
          requests,
          commands,
          droppedSavedAck: false,
          rejectedHistory: false,
          httpChatRequests: 0,
        };
        scenarios.push(scenario);
        const conversation = {
          id: "public-conversation",
          subject_id: subject,
          participant_account_id: "owner",
          last_user_message_id: null as string | null,
        };
        // Replace only messaging frames; the existing server still handles Worker subscriptions.
        // This is a synthetic protocol peer, never evidence of Backend persistence/authorization.
        await context.routeWebSocket("**/protocol/ws", (socket) => {
          const server = socket.connectToServer();
          socket.onMessage((data) => {
            const frame = JSON.parse(String(data));
            if (frame.frame !== "messaging_request") return server.send(data);
            const command = frame.message.command as MessagingCommand;
            commands.push(command);
            let result: MessagingResult;
            if (command.kind === "unsubscribe") {
              result = { kind: "unsubscribed", subscription_id: command.subscription_id };
            } else if (faultMode === "forbidden") {
              assertEquals(command.kind, "history");
              rejectedHistory = scenario.rejectedHistory = true;
              result = { kind: "rejected", status: 403 };
            } else if (command.kind === "open") {
              assertEquals(command.request.subject_id, subject);
              result = { kind: "opened", conversation };
            } else {
              assertEquals(command.selector.subject_id, subject);
              assertEquals(command.selector.conversation_id, conversation.id);
              const after = command.selector.after_sequence;
              const page = {
                conversation,
                messages: messages.filter((m) => m.sequence > after).slice(0, 100),
                has_more: messages.filter((m) => m.sequence > after).length > 100,
              };
              if (command.kind === "post") {
                const body = command.request;
                requests.push(body);
                const saved = messages.find((m) => m.operation_id === body.operation_id);
                if (saved) {
                  assertEquals(
                    body,
                    requests.find((r) => r.operation_id === body.operation_id),
                  );
                }
                const message: ChatMessage = saved ?? {
                  ...body,
                  id: `m${messages.length + 1}`,
                  conversation_id: conversation.id,
                  sequence: messages.length + 1,
                  sender: { kind: "user", account_id: "owner" },
                  last_user_message_id: `m${messages.length + 1}`,
                };
                if (!saved) {
                  messages.push(message);
                  conversation.last_user_message_id = message.id;
                }
                if (faultMode === "ack-loss" && !droppedSavedAck) {
                  // Persistence happens first, then the saved acknowledgement is lost.
                  droppedSavedAck = scenario.droppedSavedAck = true;
                  socket.close({ code: 1011, reason: "synthetic saved acknowledgement loss" });
                  return;
                }
                result = {
                  kind: "saved",
                  response: {
                    message,
                    delivery: { state: "accepted", runtime_id: "r", worker_id: "w" },
                  },
                };
              } else if (command.kind === "subscribe") {
                result = {
                  kind: "subscribed",
                  subscription_id: "fixture-conversation",
                  selector: command.selector,
                  page,
                };
              } else {
                result = { kind: "history", page };
              }
            }
            socket.send(
              JSON.stringify({
                protocol_version: 2,
                frame: "messaging_response",
                message: { request_id: frame.message.request_id, result },
              }),
            );
          });
        });
        const response = await page.goto(`${base}${subjectRoute}`);
        assertEquals(response?.status(), 200);
        await page.getByRole("heading", { level: 1, name: "Empty ready subject" }).waitFor();
        await capture(page, `${prefix}-subject`);
        if (mode === "before") continue;
        beginNavigation();
        await page.getByRole("link", { name: "Open conversation" }).click();
        await page.getByText("No public messages yet.").waitFor();
        leaving = false;
        const input = page.getByRole("textbox", { name: "Message to Empty ready subject" });
        await input.focus();
        await capture(page, `${prefix}-empty-keyboard`);
        faultMode = "ack-loss";
        const original = "  Keep the exact original input.\n日本語の本文 " +
          "long-unbroken-value-".repeat(12);
        await input.fill(original);
        await page.getByRole("button", { name: "Send message", exact: true }).click();
        await page.getByRole("alert").filter({ hasText: /^Delivery could not be confirmed\.$/ })
          .waitFor();
        await input.fill("Next draft stays editable, but cannot replace the unknown send.");
        assertEquals(
          requests[0].text,
          original,
          "the complete original text must reach persistence",
        );
        assertEquals(messages.length, 1, "ACK loss must happen after synthetic persistence");
        assert(await page.getByRole("button", { name: "Send message", exact: true }).isDisabled());
        await page.getByRole("button", { name: "Retry message", exact: true })
          .scrollIntoViewIfNeeded();
        await capture(page, `${prefix}-unknown-retained`);
        faultMode = "normal";
        const secondTab = await context.newPage();
        try {
          await secondTab.goto(`${base}${subjectRoute}/chat`);
          await secondTab.getByRole("textbox").fill("Independent input from another tab");
          await secondTab.getByRole("button", { name: "Send message", exact: true }).click();
          await secondTab.getByRole("list", { name: "Conversation messages" })
            .getByText("delivered", { exact: true }).waitFor();
          assertEquals(requests.length, 2);
          assert(requests[0].operation_id !== requests[1].operation_id);
          assert(
            await page.getByRole("button", { name: "Send message", exact: true }).isDisabled(),
          );
        } finally {
          await secondTab.close();
        }
        beginNavigation();
        await page.reload();
        const history = page.getByRole("list", { name: "Conversation messages" });
        await history.getByRole("button", { name: "Retry message", exact: true }).waitFor();
        assertEquals(
          await history.getByRole("listitem").count(),
          2,
          "reload must restore the pending message without duplicating its saved bubble",
        );
        assert(await page.getByRole("button", { name: "Send message", exact: true }).isDisabled());
        faultMode = "normal";
        leaving = false;
        await page.getByRole("button", { name: "Retry message", exact: true }).click();
        await history.getByText("delivered", { exact: true }).waitFor();
        assertEquals(requests.length, 3);
        assertEquals(requests[0], requests[2]);
        assertEquals(messages.length, 2);
        messages.push({
          id: "m3",
          conversation_id: conversation.id,
          sequence: 3,
          sender: { kind: "subject", subject_id: subject },
          text: "Explicit public reply — not internal assistant output.\n" +
            "long-unbroken-value-".repeat(16),
          reply_to: "m1",
          work_ref: null,
          operation_id: "subject-publish",
          last_user_message_id: "m2",
        });
        await page.getByRole("button", { name: "Refresh history", exact: true }).click();
        await page.getByText(/Explicit public reply/).waitFor();
        assertEquals(await history.getByRole("listitem").count(), 3);
        assertEquals(
          await history.getByRole("button", { name: "Retry message", exact: true }).count(),
          0,
        );
        await input.scrollIntoViewIfNeeded();
        await capture(page, `${prefix}-saved-and-accepted`);
        faultMode = "forbidden";
        await page.getByRole("button", { name: "Refresh history", exact: true }).click();
        await page.getByRole("alert").filter({ hasText: /do not have access/ }).waitFor();
        assertEquals(await page.getByRole("textbox").count(), 0);
        assertEquals(await history.count(), 0, "permission withdrawal must remove visible history");
        await capture(page, `${prefix}-permission-denied`);
        scenario.httpChatRequests = httpChatRequests;
        assert(
          droppedSavedAck,
          "the unknown outcome must follow a saved synthetic message and dropped ACK",
        );
        assert(rejectedHistory, "the denied UI must follow a protocol 403");
        assertEquals(httpChatRequests, 0, "conversation must not silently fall back to HTTP");
      } catch (e) {
        failures.push(`${prefix}: ${e instanceof Error ? e.stack : e}`);
        await capture(page, `${prefix}-failure`).catch(() => {});
        throw e;
      } finally {
        beginNavigation();
        await context.close();
      }
    }
  }
  assertEquals(faults.filter((f) => !f.expected), []);
  manifest.status = mode === "before"
    ? "baseline-captured-unreviewed"
    : "browser-assertions-passed-unreviewed";
} catch (e) {
  manifest.status = "failed";
  throw e;
} finally {
  try {
    await browser?.close();
  } finally {
    await stopOwnedProcesses(owned);
    await save();
    console.log(join(output, "review-context.json"));
  }
}
