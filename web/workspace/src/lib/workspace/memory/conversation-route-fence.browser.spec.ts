// @vitest-environment happy-dom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import SubjectConversation from "./SubjectConversation.svelte";
import { browserSession } from "#lib/workspace/auth/session-state.ts";
import { authUser } from "#lib/workspace/auth/api.test-fixtures.ts";
import { readConversationAttempt } from "./conversation-attempt.ts";
const props = {
  workspaceId: "w",
  subjectId: "s",
  accountId: authUser.account_id,
  subjectName: "Companion",
};
beforeEach(() => {
  sessionStorage.clear();
  localStorage.clear();
  browserSession.snapshot = {
    kind: "ready",
    value: {
      actor: { ...authUser, auth_method: "browser_session" },
      authentication_reference: "fixture-session",
      session: { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" },
    },
  };
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  sessionStorage.clear();
  localStorage.clear();
});
function conversation(subject: string) {
  return {
    id: `c-${subject}`,
    subject_id: subject,
    participant_account_id: props.accountId,
    last_user_message_id: null,
  };
}
function page(subject: string, text: string) {
  return {
    conversation: conversation(subject),
    messages: [{
      id: `m-${subject}`,
      conversation_id: `c-${subject}`,
      sequence: 1,
      sender: { kind: "subject", subject_id: subject },
      text,
      reply_to: null,
      work_ref: null,
      operation_id: "publish",
      last_user_message_id: null,
    }],
    has_more: false,
  };
}
for (const status of [401, 403, 404]) {
  test(`${status} withdraws retained text and fences a late POST ACK without deleting its retry record`, async () => {
    let release!: (response: Response) => void;
    let posted = false;
    let denied = false;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: string, init?: RequestInit) => {
        if (String(input).endsWith("/conversations")) return Response.json(conversation("s"));
        if (init?.method === "POST") {
          posted = true;
          return await new Promise<Response>((resolve) => {
            release = resolve;
          });
        }
        return denied
          ? new Response(null, { status })
          : Response.json({ conversation: conversation("s"), messages: [], has_more: false });
      }),
    );
    render(SubjectConversation, props);
    await screen.findByText("No public messages yet.");
    await fireEvent.input(screen.getByRole("textbox"), {
      target: { value: "retained private text" },
    });
    await fireEvent.click(screen.getByRole("button", { name: "Send message" }));
    await waitFor(() => expect(posted).toBe(true));
    const attempt = readConversationAttempt(sessionStorage, props)!;
    denied = true;
    await fireEvent.click(screen.getByRole("button", { name: "Refresh history" }));
    await screen.findByRole("alert");
    expect(screen.queryByText("retained private text")).toBeNull();
    expect(screen.queryByRole("textbox")).toBeNull();
    release(Response.json({
      message: {
        id: "user",
        conversation_id: attempt.conversationId,
        sequence: 1,
        sender: { kind: "user", account_id: props.accountId },
        text: attempt.text,
        reply_to: null,
        work_ref: null,
        operation_id: attempt.operationId,
        last_user_message_id: "user",
      },
      delivery: { state: "accepted", runtime_id: "r", worker_id: "w" },
    }));
    await Promise.resolve();
    expect(readConversationAttempt(sessionStorage, props)).toEqual(attempt);
    expect(
      screen.queryByText("Runtime accepted this input. Processing and replies are not confirmed."),
    ).toBeNull();
    expect(screen.queryByText("Loading conversation…")).toBeNull();
  });
}

for (const changed of [{ subjectId: "other" }, { workspaceId: "other" }]) {
  test(`late history cannot populate a different ${Object.keys(changed)[0]} route`, async () => {
    let release!: (response: Response) => void;
    let first = true;
    let readingOld = false;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (_input: string, init?: RequestInit) => {
        if (init?.method === "POST") {
          return Response.json(conversation(JSON.parse(String(init.body)).subject_id));
        }
        if (first) {
          first = false;
          readingOld = true;
          return await new Promise<Response>((resolve) => {
            release = resolve;
          });
        }
        return Response.json(page(changed.subjectId ?? "s", "current route public reply"));
      }),
    );
    const view = render(SubjectConversation, props);
    await waitFor(() => expect(readingOld).toBe(true));
    await view.rerender({ ...props, ...changed });
    await screen.findByText("current route public reply");
    release(Response.json(page("s", "old route secret reply")));
    await Promise.resolve();
    expect(screen.queryByText("old route secret reply")).toBeNull();
  });
}

test("authentication change while POST is in flight never clears the former Account's retained input or adopts its ACK", async () => {
  let release!: (response: Response) => void;
  let posted = false;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string, init?: RequestInit) => {
      if (String(input).endsWith("/conversations")) return Response.json(conversation("s"));
      if (init?.method === "POST") {
        posted = true;
        return await new Promise<Response>((resolve) => {
          release = resolve;
        });
      }
      return Response.json({ conversation: conversation("s"), messages: [], has_more: false });
    }),
  );
  render(SubjectConversation, props);
  await screen.findByText("No public messages yet.");
  await fireEvent.input(screen.getByRole("textbox"), { target: { value: "former account input" } });
  await fireEvent.click(screen.getByRole("button", { name: "Send message" }));
  await waitFor(() => expect(posted).toBe(true));
  const attempt = readConversationAttempt(sessionStorage, props)!;
  browserSession.invalidate();
  release(Response.json({
    message: {
      id: "user",
      conversation_id: attempt.conversationId,
      sequence: 1,
      sender: { kind: "user", account_id: props.accountId },
      text: attempt.text,
      reply_to: null,
      work_ref: null,
      operation_id: attempt.operationId,
      last_user_message_id: "user",
    },
    delivery: { state: "accepted", runtime_id: "r", worker_id: "w" },
  }));
  await Promise.resolve();
  expect(readConversationAttempt(sessionStorage, props)).toEqual(attempt);
  expect(
    screen.queryByText("Runtime accepted this input. Processing and replies are not confirmed."),
  ).toBeNull();
  expect(screen.queryByRole("textbox")).toBeNull();
  expect(readConversationAttempt(sessionStorage, { ...props, accountId: "other" })).toBeNull();
});
