// @vitest-environment happy-dom
import { cleanup, fireEvent, render, waitFor } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import type { ChatMessage } from "#lib/generated/chat-api.ts";
import ConversationHistory from "./ConversationHistory.svelte";
import {
  conversationDraftKey,
  readConversationDraft,
  saveConversationDraft,
} from "./conversation-draft.ts";
import { conversationMessageView } from "./conversation-view.ts";
import {
  captureTranscriptAnchor,
  followTranscriptTail,
  restoreTranscriptAnchor,
} from "#lib/workspace/transcript/scroll.ts";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  sessionStorage.clear();
});
const props = { accountId: "a", subjectName: "Companion" };
function message(n: number, text = `reply ${n}`, own = false): ChatMessage {
  return {
    id: `m${n}`,
    conversation_id: "c",
    sequence: n,
    text,
    sender: own ? { kind: "user", account_id: "a" } : { kind: "subject", subject_id: "s" },
    operation_id: `op${n}`,
    reply_to: null,
    work_ref: null,
    last_user_message_id: null,
  };
}

test("public messages use identity-based bubbles and safe shared Markdown, never sender-kind alone for ownership", async () => {
  const own = message(1, "**own**\n\n`code`", true);
  const other = { ...message(2), sender: { kind: "user" as const, account_id: "different" } };
  expect(conversationMessageView(other, props.accountId, props.subjectName).own).toBe(false);
  const ui = render(ConversationHistory, {
    ...props,
    messages: [
      own,
      other,
      message(3, "<script>alert(1)</script>\n\n[unsafe](javascript:alert(1))"),
    ],
  });
  await waitFor(() => expect(ui.container.querySelector(".own strong")?.textContent).toBe("own"));
  expect(ui.container.querySelectorAll(".chat-message.own")).toHaveLength(1);
  expect(ui.container.querySelector(".own code")?.textContent).toBe("code");
  expect(ui.container.querySelector("script")).toBeNull();
  expect(ui.container.querySelector('a[href^="javascript:"]')).toBeNull();
});

test("new messages follow only at the tail and jump-to-latest resumes following", async () => {
  let height = 1000;
  vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(() => height);
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(200);
  const ui = render(ConversationHistory, { ...props, messages: [message(1)] });
  const scroll = ui.getByRole("region", { name: "Conversation history" });
  await waitFor(() => expect(scroll.scrollTop).toBe(1000));
  scroll.scrollTop = 200;
  await fireEvent.scroll(scroll);
  height = 1400;
  await ui.rerender({ messages: [message(1), message(2)] });
  expect(scroll.scrollTop).toBe(200);
  await fireEvent.click(ui.getByRole("button", { name: "Jump to latest" }));
  await waitFor(() => expect(scroll.scrollTop).toBe(1400));
  height = 1600;
  await ui.rerender({ messages: [message(1), message(2), message(3)] });
  await waitFor(() => expect(scroll.scrollTop).toBe(1600));
});

test("shared scroll preserves a retained row across prepend and fences a pending tail-follow", async () => {
  const container = document.createElement("div");
  const row = document.createElement("div");
  row.dataset.messageId = "first";
  container.append(row);
  document.body.append(container);
  let top = 20;
  vi.spyOn(row, "getBoundingClientRect").mockImplementation(() => ({ top }) as DOMRect);
  const anchor = captureTranscriptAnchor(container, "[data-message-id]", "messageId");
  container.scrollTop = 100;
  top = 220;
  restoreTranscriptAnchor(container, '[data-message-id="first"]', anchor);
  expect(container.scrollTop).toBe(300);
  let current = true;
  const pending = followTranscriptTail(container, () => current);
  current = false;
  expect(await pending).toBe(false);
  expect(container.scrollTop).toBe(300);
  container.remove();
});

test("draft storage isolates all identity boundaries and rejects corrupt data without replacing it", () => {
  const scope = { accountId: "a", workspaceId: "w", subjectId: "s" };
  const key = conversationDraftKey(scope, "c");
  const text = "  draft\r\n日本語  ";
  saveConversationDraft(sessionStorage, key, text);
  expect(readConversationDraft(sessionStorage, key)).toBe(text);
  for (
    const other of [{ ...scope, accountId: "b" }, { ...scope, workspaceId: "other" }, {
      ...scope,
      subjectId: "other",
    }]
  ) {
    expect(readConversationDraft(sessionStorage, conversationDraftKey(other, "c"))).toBe("");
  }
  expect(readConversationDraft(sessionStorage, conversationDraftKey(scope, "other"))).toBe("");
  sessionStorage.setItem(key, "{bad");
  expect(() => readConversationDraft(sessionStorage, key)).toThrow();
  expect(sessionStorage.getItem(key)).toBe("{bad");
});
