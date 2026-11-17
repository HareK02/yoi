/// <reference lib="dom" />
// 本番Web buildとmultiplexerを使い、外側のHTTP/WebSocketとSubject返信だけを合成する。
import { assert, assertEquals } from "@std/assert";
import { dirname, fromFileUrl, join, resolve } from "@std/path";
import { chromium } from "playwright";
import { sha256File } from "../src/artifacts.ts";
import { startOwnedProcesses, stopOwnedProcesses } from "../src/processes.ts";
import { freePort } from "./memory_document_harness.ts";
import type {
  ChatMessage,
  ChatUserMessageRequest,
  MessagingCommand,
  MessagingResult,
} from "#lib/generated/chat-api.ts";

const root = resolve(dirname(fromFileUrl(import.meta.url)), "../../..");
const mode = Deno.args[0];
if (mode !== "before" && mode !== "after") throw new Error("usage: chat_ui_review.ts before|after");
const output = join(root, "target/web-ux/t856", mode);
await Deno.mkdir(output, { recursive: true });
const build = join(root, "web/workspace/build");
const port = await freePort(), base = `http://127.0.0.1:${port}`;
const owned = await startOwnedProcesses(
  [{
    id: "chat-fixture",
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
const captures: unknown[] = [], faults: unknown[] = [];
const result = {
  evidence: "production Web + synthetic authenticated HTTP and messaging; not real Backend/Runtime",
  mode,
  status: "running",
  visualVerdict: "UNREVIEWED",
  captures,
  faults,
};
try {
  for (const colorScheme of ["light", "dark"] as const) {
    for (const width of [1440, 768, 390]) {
      const context = await browser.newContext({
        viewport: { width, height: 1000 },
        colorScheme,
        reducedMotion: "reduce",
      });
      const page = await context.newPage();
      page.setDefaultTimeout(15000);
      page.on(
        "pageerror",
        (e) => faults.push({ kind: "pageerror", message: e.message, width, colorScheme }),
      );
      page.on("console", (e) => {
        if (e.type() === "error") {
          faults.push({ kind: "console", message: e.text(), width, colorScheme });
        }
      });
      const conversation = {
        id: "chat-fixture",
        subject_id: "empty-subject",
        participant_account_id: "owner",
        last_user_message_id: null as string | null,
      };
      const messages: ChatMessage[] = [];
      const posts: ChatUserMessageRequest[] = [];
      function add(text: string, own = false, operation = `op-${messages.length}`) {
        const id = `message-${messages.length + 1}`;
        if (own) {
          conversation.last_user_message_id = id;
        }
        const message: ChatMessage = {
          id,
          conversation_id: conversation.id,
          sequence: messages.length + 1,
          sender: own
            ? { kind: "user", account_id: "owner" }
            : { kind: "subject", subject_id: "empty-subject" },
          text,
          operation_id: operation,
          reply_to: null,
          work_ref: null,
          last_user_message_id: conversation.last_user_message_id,
        };
        messages.push(message);
        return message;
      }
      add("履歴を **チャットバブル** にしたい。\n自分は右、相手は左に。", true);
      add(
        '## 表示の共有\nMarkdown、複数行、コードを表示します。\n\n- 既存の描画を再利用\n- 過去ログ閲覧中は追従しない\n\n```typescript\nconst greeting = "こんにちは";\nconsole.log(greeting);\n```',
      );
      add("長い識別子も画面幅からはみ出さないようにしてください。", true);
      add(
        "identifier_" + "long_value_".repeat(24) +
          "\n\n[安全なリンク](https://example.com) と `inline code`。",
      );
      let loseAck = false;
      let publish: () => void = () => {};
      await context.routeWebSocket("**/protocol/ws", (socket) => {
        const send = (frame: unknown) => socket.send(JSON.stringify(frame));
        publish = () =>
          send({
            protocol_version: 2,
            frame: "messaging_event",
            message: {
              kind: "messages",
              subscription_id: "chat-sub",
              page: { conversation, messages, has_more: false },
            },
          });
        socket.onMessage((raw) => {
          const frame = JSON.parse(String(raw));
          if (frame.frame === "request" && frame.message.method === "subscribe_events") {
            send({
              protocol_version: 2,
              frame: "response",
              message: {
                result: "subscribed",
                payload: {
                  request_id: frame.message.params.request_id,
                  subscription_id: "workers",
                  selector: { topic: "workspace_workers" },
                  snapshot: { topic: "workers", data: { workers: [] } },
                },
              },
            });
            return;
          }
          if (frame.frame !== "messaging_request") return;
          const command = frame.message.command as MessagingCommand;
          let response: MessagingResult;
          if (command.kind === "open") response = { kind: "opened", conversation };
          else if (command.kind === "subscribe") {
            response = {
              kind: "subscribed",
              subscription_id: "chat-sub",
              selector: command.selector,
              page: {
                conversation,
                messages: messages.filter((m) => m.sequence > command.selector.after_sequence),
                has_more: false,
              },
            };
          } else if (command.kind === "history") {
            response = {
              kind: "history",
              page: {
                conversation,
                messages: messages.filter((m) => m.sequence > (command.query.after_sequence ?? 0)),
                has_more: false,
              },
            };
          } else if (command.kind === "post") {
            posts.push(command.request);
            const message = messages.find((m) => m.operation_id === command.request.operation_id) ??
              add(command.request.text, true, command.request.operation_id);
            response = {
              kind: "saved",
              response: {
                message,
                delivery: { state: "accepted", runtime_id: "fixture", worker_id: "fixture" },
              },
            };
            if (loseAck) {
              loseAck = false;
              socket.close();
              return;
            }
          } else return;
          send({
            protocol_version: 2,
            frame: "messaging_response",
            message: { request_id: frame.message.request_id, result: response },
          });
          if (command.kind === "post") publish();
        });
      });
      const response = await page.goto(`${base}/w/memory-review/subjektiv/empty-subject/chat`);
      assertEquals(response?.status(), 200);
      await page.getByRole("textbox").waitFor();
      await page.getByText("Conversation connection: connected").waitFor();
      if (mode === "after") {
        assertEquals(await page.locator(".chat-message.own").count(), 2);
        assertEquals(await page.locator(".chat-message:not(.own)").count(), 2);
        assert(await page.locator(".chat-bubble strong").count() > 0);
        assert(await page.locator(".chat-bubble pre code").count() > 0);
        assertEquals(await page.getByRole("button", { name: /cancel/i }).count(), 0);
        const geometry = await page.locator(".chat-message").evaluateAll((rows) =>
          rows.map((row) => {
            const outer = row.getBoundingClientRect(),
              bubble = row.firstElementChild!.getBoundingClientRect();
            return {
              own: row.classList.contains("own"),
              left: bubble.left - outer.left,
              right: outer.right - bubble.right,
            };
          })
        );
        assert(geometry.every((row) => row.own ? row.right < 2 : row.left < 2));
        const layout = await page.evaluate(() => {
          const history = document.querySelector('.chat-scroll')!.getBoundingClientRect();
          const main = document.querySelector('.app-shell__main')!.getBoundingClientRect();
          const shell = document.querySelector('.subject-conversation .composer-input-shell')!;
          const send = document.querySelector('.subject-conversation .composer-send-button')!;
          return { historyHeight: history.height, availableHeight: main.height,
            composerBottom: shell.getBoundingClientRect().bottom,
            radius: getComputedStyle(shell).borderRadius,
            sendWidth: send.getBoundingClientRect().width,
            sendHeight: send.getBoundingClientRect().height,
            pageOverflow: document.querySelector('.app-shell__main')!.scrollHeight > document.querySelector('.app-shell__main')!.clientHeight + 1 };
        });
        assert(layout.historyHeight > layout.availableHeight * .6, JSON.stringify(layout));
        assert(layout.composerBottom <= 1000 && !layout.pageOverflow, JSON.stringify(layout));
        assertEquals(layout.radius, '18px');
        assertEquals(layout.sendWidth, layout.sendHeight);
        assertEquals(await page.locator('.body-controls').getAttribute('open'), null);
        await page.getByText('Subject controls', { exact: true }).click();
        await page.getByRole('button', { name: 'Start Subject', exact: true }).waitFor();
        assertEquals(await page.getByRole('button', { name: 'Refresh body state', exact: true }).count(), 0);
        assertEquals(await page.getByText('Request diagnostics', { exact: true }).count(), 0);
        await page.getByText('Subject controls', { exact: true }).click();
      }
      await page.locator(".subject-conversation").scrollIntoViewIfNeeded();
      const path = join(output, `${width}-${colorScheme}.png`);
      await page.screenshot({ path, fullPage: true });
      captures.push({
        width,
        colorScheme,
        path,
        sha256: await sha256File(path),
        documentStatus: response?.status(),
        accessibility: await page.locator(".subject-conversation").ariaSnapshot(),
        overflow: await page.evaluate(() => document.documentElement.scrollWidth > innerWidth),
      });
      if (mode === "after") {
        const editor = page.getByRole("textbox");
        const shell = page.locator('.subject-conversation .composer-input-shell');
        const shellState = () => shell.evaluate(node => {
          const style = getComputedStyle(node);
          const rect = node.getBoundingClientRect();
          const probe = document.createElement('span');
          probe.style.color = 'var(--line)';
          node.append(probe);
          const lineColor = getComputedStyle(probe).color;
          probe.remove();
          return { border: style.borderTopColor, shadow: style.boxShadow, lineColor, width: rect.width, height: rect.height };
        });
        const unfocused = await shellState();
        assertEquals(unfocused.border, 'rgba(0, 0, 0, 0)');
        assertEquals(unfocused.shadow, 'none');
        await editor.focus();
        const focused = await shellState();
        assertEquals(focused.border, focused.lineColor);
        assertEquals(focused.shadow, 'none');
        assertEquals([focused.width, focused.height], [unfocused.width, unfocused.height]);
        const focusPath = join(output, `${width}-${colorScheme}-focus.png`);
        await page.screenshot({ path: focusPath, fullPage: true });
        captures.push({ width, colorScheme, state: 'composer-focused', path: focusPath, sha256: await sha256File(focusPath) });
        await page.getByRole('region', { name: 'Conversation history', exact: true }).focus();
        assertEquals(await shellState(), unfocused);
        const text = "  日本語の長い貼り付け\n" + "full paste 日本語\n".repeat(80);
        await editor.evaluate((node, value) => {
          const data = new DataTransfer();
          data.setData("text/plain", value);
          node.dispatchEvent(
            new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true }),
          );
        }, text);
        await page.getByRole("button", { name: "Send message", exact: true }).click();
        await page.locator('.chat-message').filter({ hasText: '日本語の長い貼り付け' }).locator('.chat-delivery').getByText('delivered', { exact: true }).waitFor();
        assertEquals(posts[0]?.text, text);
        await editor.fill("next draft");
        await page.waitForFunction(() =>
          Object.keys(sessionStorage).some((key) =>
            key.startsWith("yoi.conversation-draft:") &&
            sessionStorage.getItem(key) === JSON.stringify("next draft")
          )
        );
        console.log("draft persisted", width, colorScheme);
        await page.reload();
        await page.getByRole("textbox").waitFor();
        await page.waitForFunction(() =>
          document.querySelector("[contenteditable=true]")?.textContent === "next draft"
        );
        console.log("draft restored", width, colorScheme);
        const scroll = page.locator(".chat-scroll");
        const beforeJumpHeight = await scroll.evaluate(node => node.clientHeight);
        await scroll.evaluate((node) => {
          node.scrollTop = 0;
          node.dispatchEvent(new Event("scroll"));
        });
        await page.getByRole("button", { name: "Jump to latest" }).waitFor();
        assertEquals(await scroll.evaluate(node => node.clientHeight), beforeJumpHeight);
        const jumpBox = await page.getByRole('button', { name: 'Jump to latest' }).boundingBox();
        const scrollBox = (await scroll.boundingBox())!;
        assert(jumpBox && jumpBox.width === 36 && jumpBox.height === 36);
        assert(jumpBox.y >= scrollBox.y && jumpBox.y + jumpBox.height <= scrollBox.y + scrollBox.height);
        const jumpPath = join(output, `${width}-${colorScheme}-jump.png`);
        await page.screenshot({ path: jumpPath, fullPage: true });
        captures.push({ width, colorScheme, state: 'reading-history', path: jumpPath, sha256: await sha256File(jumpPath) });
        if (width === 390) {
          await page.setViewportSize({ width, height: 600 });
          const compact = await page.locator('.subject-conversation .composer-input-shell').boundingBox();
          assert(compact && compact.y + compact.height <= 600);
          assert(await scroll.evaluate(node => node.clientHeight) > 200);
          const compactPath = join(output, `${width}-${colorScheme}-short.png`);
          await page.screenshot({ path: compactPath, fullPage: true });
          captures.push({ width, height: 600, colorScheme, state: 'short-viewport', path: compactPath, sha256: await sha256File(compactPath) });
          await page.setViewportSize({ width, height: 1000 });
        }
        add("new reply while reading history");
        publish();
        await page.getByText("new reply while reading history", { exact: true }).waitFor();
        assertEquals(await scroll.evaluate((node) => node.scrollTop), 0);
        await page.getByRole("button", { name: "Jump to latest" }).click();
        await page.waitForFunction(() => {
          const n = document.querySelector(".chat-scroll")!;
          return n.scrollHeight - n.scrollTop - n.clientHeight < 2;
        });
        await editor.fill(":cancel");
        await editor.press("Control+Enter");
        await page.waitForFunction(() =>
          document.querySelector("[contenteditable=true]")?.textContent === ""
        );
        assertEquals(posts.at(-1)?.text, ":cancel");
        loseAck = true;
        await editor.fill("ACK loss retains this exact input");
        await page.getByRole("button", { name: "Send message", exact: true }).click();
        await page.getByRole("button", { name: "Retry message" }).waitFor();
        const retained = structuredClone(posts.at(-1)!);
        await editor.fill("editable next draft while outcome is unknown");
        assert(await page.getByRole("button", { name: "Send message", exact: true }).isDisabled());
        await page.getByText("Conversation connection: connected").waitFor();
        if (width === 390) {
          const path = join(output, `${width}-${colorScheme}-unknown.png`);
          await page.screenshot({ path, fullPage: true });
          captures.push({
            width,
            colorScheme,
            state: "saved-but-ACK-lost",
            path,
            sha256: await sha256File(path),
            accessibility: await page.locator(".subject-conversation").ariaSnapshot(),
          });
        }
        await page.getByRole("button", { name: "Retry message" }).click();
        await page.locator('.chat-message').filter({ hasText: retained.text }).locator('.chat-delivery').getByText('delivered', { exact: true }).waitFor();
        assertEquals(await page.getByRole('button', { name: 'Retry message' }).count(), 0);
        assertEquals(await page.getByText('Runtime accepted this input. Processing and replies are not confirmed.', { exact: true }).count(), 0);
        const deliveredPath = join(output, `${width}-${colorScheme}-delivered.png`);
        await page.screenshot({ path: deliveredPath, fullPage: true });
        captures.push({ width, colorScheme, state: 'delivered', path: deliveredPath, sha256: await sha256File(deliveredPath) });
        assertEquals(posts.at(-1), retained);
        assertEquals(
          messages.filter((message) => message.operation_id === retained.operation_id).length,
          1,
        );
        assertEquals(await editor.textContent(), "editable next draft while outcome is unknown");
      }
      await context.close();
    }
  }
  assertEquals(faults, []);
  result.status = "pass";
} catch (error) {
  result.status = "fail";
  throw error;
} finally {
  await Deno.writeTextFile(join(output, "review-context.json"), JSON.stringify(result, null, 2));
  await browser.close();
  await stopOwnedProcesses(owned);
}
