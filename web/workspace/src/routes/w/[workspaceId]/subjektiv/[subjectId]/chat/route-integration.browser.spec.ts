// @vitest-environment happy-dom
import { cleanup, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import PageContent from "./PageContent.svelte";
import { browserSession } from "#lib/workspace/auth/session-state.ts";
import { authUser } from "#lib/workspace/auth/api.test-fixtures.ts";

const calls: string[] = [];
function routeData(
  workspaceId = "w",
  subjectId = "subject-a",
  accountId = authUser.account_id,
) {
  return {
    workspaceId,
    subjectId,
    accountId,
    subject: {
      data: { id: subjectId, role: `Companion ${subjectId}`, state: "active" },
      error: null,
    },
  };
}
beforeEach(() => {
  calls.length = 0;
  localStorage.clear();
  sessionStorage.clear();
  browserSession.snapshot = {
    kind: "ready",
    value: {
      actor: { ...authUser, auth_method: "browser_session" },
      authentication_reference: "route-integration-fixture",
      session: { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" },
    },
  };
  vi.stubGlobal("fetch", (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(String(input), "https://fixture.invalid");
    calls.push(`${init?.method ?? "GET"} ${url.pathname}`);
    const conversation = {
      id: "conversation",
      subject_id: "subject-a",
      participant_account_id: authUser.account_id,
      last_user_message_id: null,
    };
    if (url.pathname.endsWith("/body")) {
      return Promise.resolve(Response.json({
        workspace_id: "w",
        subject_id: "subject-a",
        current: null,
        observation: { kind: "absent" },
        can_start: true,
        can_stop: true,
      }));
    }
    if (url.pathname.endsWith("/conversations")) {
      return Promise.resolve(Response.json(conversation));
    }
    return Promise.resolve(
      Response.json({ conversation, messages: [], has_more: false }),
    );
  });
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  sessionStorage.clear();
  localStorage.clear();
});

test("canonical chat hosts Body and public conversation as siblings with a canonical return link", async () => {
  render(PageContent, { data: routeData() } as never);
  await screen.findByText("No current conversation body.");
  await screen.findByRole("textbox", {
    name: "Message to Companion subject-a",
  });
  expect(
    screen.getByRole("link", { name: "← Subject details" }).getAttribute(
      "href",
    ),
  ).toBe("/w/w/subjektiv/subject-a");
  const body = screen.getByRole("heading", { name: "Conversation body" })
    .closest("section")!;
  const conversation = screen.getByRole("textbox").closest("section")!;
  expect(body.parentElement).toBe(conversation.parentElement);
  expect(body.contains(conversation)).toBe(false);
  expect(calls).toContain("GET /api/w/w/subjektiv/subjects/subject-a/body");
  expect(calls.some((call) => call.includes("/api/w/w/chat/conversations")))
    .toBe(true);
  expect(calls.some((call) => /\/body\/(start|stop)/.test(call))).toBe(false);
});

test("auth-generation change removes both children even when the account snapshot is preserved", async () => {
  render(PageContent, { data: routeData() } as never);
  await screen.findByRole("textbox");
  browserSession.invalidate(true);
  await screen.findByRole("alert");
  expect(screen.queryByRole("textbox")).toBeNull();
  expect(screen.queryByRole("heading", { name: "Conversation body" }))
    .toBeNull();
  expect(screen.getByRole("alert").textContent).toContain("Reopen this page");
});

test("route reuse with an unconfirmed account never mounts controls or reads the new Subject", async () => {
  const view = render(PageContent, { data: routeData() } as never);
  await screen.findByRole("textbox");
  calls.length = 0;
  await view.rerender(
    {
      data: routeData(
        "other/workspace",
        "other/subject",
        "unconfirmed-account",
      ),
    } as never,
  );
  await waitFor(() => expect(screen.queryByRole("textbox")).toBeNull());
  expect(screen.queryByRole("heading", { name: "Conversation body" }))
    .toBeNull();
  expect(
    screen.getByRole("link", { name: "← Subject details" }).getAttribute(
      "href",
    ),
  ).toBe("/w/other%2Fworkspace/subjektiv/other%2Fsubject");
  expect(calls.some((call) => call.includes("other"))).toBe(false);
});
