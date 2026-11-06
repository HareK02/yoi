// @vitest-environment happy-dom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import { goto } from "$app/navigation";
import type { ComponentProps } from "svelte";
import { startupFixture } from "#lib/workspace/workers/startup-failure.test-fixtures.ts";
import NewWorker from "./PageContent.svelte";
function mount(
  ticketContext:
    | { ticketId: string; ticketTitle: string; initialInput: string }
    | null = null,
) {
  const data = {
    workspaceId: "empty",
    repositories: {
      workspace_id: "empty",
      items: [],
      source: "fixture",
      diagnostics: [],
    },
    ticketContext,
  } as unknown as ComponentProps<typeof NewWorker>["data"];
  return render(NewWorker, {
    props: { data },
  });
}
import { emptyLaunchOptions } from "#lib/workspace/sidebar/worker-launch.test-fixtures.ts";

vi.mock("$app/navigation", () => ({ goto: vi.fn(async () => {}) }));
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

test("zero-repository Workspace launches a normal embedded Worker request without Workdir or Ticket assignment", async () => {
  const api = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) =>
    String(input).endsWith("/launch-options")
      ? Response.json(emptyLaunchOptions("empty"))
      : Response.json({
        error: "unavailable",
        message: "Retry later",
        diagnostics: [],
      }, { status: 503 })
  );
  vi.stubGlobal("fetch", api);
  mount();
  const start = await screen.findByRole("button", { name: "Start Worker" });
  expect((start as HTMLButtonElement).disabled).toBe(false);
  expect(
    screen.getByText(
      "No filesystem tools or Bash will be available without a Workdir.",
    ),
  ).toBeTruthy();
  expect(screen.queryByLabelText("Attachment 1 Workdir")).toBeNull();
  await fireEvent.click(start);
  await waitFor(() => expect(api).toHaveBeenCalledTimes(2));
  const body = JSON.parse(String(api.mock.calls[1][1]?.body));
  expect(body).toMatchObject({
    runtime_id: "embedded",
    profile: null,
    workdir_attachments: [],
    ticket_assignment: null,
  });
  expect(body).not.toHaveProperty("repository_key");
});

test("zero-repository Workspace allows a remote Runtime launch without attachments", async () => {
  const options = emptyLaunchOptions("empty");
  vi.stubGlobal(
    "fetch",
    vi.fn(async () =>
      Response.json({ ...options, runtimes: [...options.runtimes].reverse() })
    ),
  );
  mount();
  const start = await screen.findByRole("button", { name: "Start Worker" });
  expect(screen.queryByLabelText("Attachment 1 Workdir")).toBeNull();
  expect(screen.getByRole("button", { name: "Add attachment" })).toBeTruthy();
  expect((start as HTMLButtonElement).disabled).toBe(false);
});

test.each(["embedded", "remote"].flatMap((runtime) =>
  [
    "Review is unnecessary.",
    "Review, then merge if approved.",
    "Return without a conclusion.",
  ].map((text) => ({ runtime, text }))
))(
  "$runtime Ticket launch retains natural request without attachments: $text",
  async ({ runtime, text }) => {
    const options = emptyLaunchOptions("empty");
    if (runtime === "remote") options.runtimes.reverse();
    options.profiles.push({
      ...options.profiles[0],
      id: "builtin:reviewer",
      label: "Trusted Reviewer",
    });
    const api = vi.fn(async (input: RequestInfo | URL, _init?: RequestInit) =>
      String(input).endsWith("/launch-options")
        ? Response.json(options)
        : Response.json({
          error: "permission_denied",
          message: "Backend rejected this assignment",
          diagnostics: [],
        }, { status: 403 })
    );
    vi.stubGlobal("fetch", api);
    mount({
      ticketId: "T-720",
      ticketTitle: "Investigate launch",
      initialInput: text,
    });
    const start = await screen.findByRole("button", { name: "Start Worker" });
    expect((start as HTMLButtonElement).disabled).toBe(false);
    expect(screen.queryByRole("option", { name: "Trusted Reviewer" }))
      .toBeNull();
    await fireEvent.click(start);
    expect((await screen.findByRole("alert")).textContent).toContain(
      "The request did not return a supported failure response. The operation result has not been confirmed.",
    );
    const request = JSON.parse(String(api.mock.calls[1][1]?.body));
    expect(request.runtime_id).toBe(runtime);
    expect(request.ticket_assignment.ticket_id).toBe("T-720");
    expect(request.ticket_assignment.operation_id).toMatch(/^[0-9a-f-]{36}$/);
    expect(request.initial_submit).toEqual([{ kind: "text", content: text }]);
    expect(request.profile).toBeNull();
    expect(request.workdir_attachments).toEqual([]);
    expect(request.ticket_assignment).not.toHaveProperty("role");
  },
);

test("read-only Workdir requires an explicit selection and keeps backend permissions authoritative", async () => {
  const options = emptyLaunchOptions("empty");
  options.runtimes.reverse();
  options.working_directories = [{
    working_directory_id: "read-only-grant",
    display_name: "Read-only sessions",
    source: {
      kind: "external_grant",
      grant_id: "grant-1",
      grant_permissions: { read: true, write: false, command: false },
    },
    materializer_kind: "client_hosted_external",
    status: "active",
    cleanliness: "unknown",
  }];
  const api = vi.fn(async (input: RequestInfo | URL, _init?: RequestInit) =>
    String(input).endsWith("/launch-options")
      ? Response.json(options)
      : Response.json({
        error: "permission_denied",
        message: "No write permission",
        diagnostics: [],
      }, { status: 403 })
  );
  vi.stubGlobal("fetch", api);
  mount({
    ticketId: "T-720",
    ticketTitle: "Read-only investigation",
    initialInput: "Return without a conclusion.",
  });
  const start = await screen.findByRole("button", {
    name: "Start Worker",
  }) as HTMLButtonElement;
  expect(start.disabled).toBe(false);
  await fireEvent.click(screen.getByRole("button", { name: "Add attachment" }));
  const select = await screen.findByLabelText("Attachment 1 Workdir");
  expect((select as HTMLSelectElement).value).toBe("");
  expect(start.disabled).toBe(true);
  await fireEvent.change(select, { target: { value: "read-only-grant" } });
  await waitFor(() => expect(start.disabled).toBe(false));
  await fireEvent.click(start);
  expect((await screen.findByRole("alert")).textContent).toContain(
    "The request did not return a supported failure response. The operation result has not been confirmed.",
  );
  const request = JSON.parse(String(api.mock.calls[1][1]?.body));
  expect(request.workdir_attachments).toEqual([{
    alias: "workdir",
    working_directory_id: "read-only-grant",
    relative_cwd: null,
  }]);
  expect(request).not.toHaveProperty("targets");
  expect(request.workdir_attachments[0]).not.toHaveProperty("permissions");
});

test.each(["backend", "network"])(
  "explicit Flow stays optional and uncertain %s outcome prevents duplicate launch",
  async (failure) => {
    const api = vi.fn(async (input: RequestInfo | URL, _init?: RequestInit) => {
      if (String(input).endsWith("/launch-options")) {
        return Response.json(emptyLaunchOptions("empty"));
      }
      if (failure === "network") {
        throw new TypeError("Connection lost after dispatch");
      }
      return Response.json({
        error: "OutcomeUnknown",
        message: "Runtime result needs reconciliation",
        diagnostics: [],
      }, { status: 503 });
    });
    vi.stubGlobal("fetch", api);
    mount({
      ticketId: "T-720",
      ticketTitle: "Investigate launch",
      initialInput: "Return without a conclusion.",
    });
    const start = await screen.findByRole("button", { name: "Start Worker" });
    await fireEvent.input(screen.getByLabelText("Flow (optional)"), {
      target: { value: "project:investigate" },
    });
    await fireEvent.click(start);
    await screen.findByText(
      "The launch result needs reconciliation. Check the Worker list or Ticket before starting another Worker.",
    );
    expect((start as HTMLButtonElement).disabled).toBe(true);
    const request = JSON.parse(String(api.mock.calls[1][1]?.body));
    expect(request.initial_submit).toEqual([{
      kind: "text",
      content: "Return without a conclusion.",
    }, { kind: "flow", selector: "project:investigate" }]);
    await fireEvent.click(start);
    expect(api).toHaveBeenCalledTimes(2);
  },
);

test.each(["denial", "lost", "rejected", "compensation"] as const)(
  "startup contract %s shows cause before details, preserves draft and applies only typed operation gating",
  async (name) => {
    const body = startupFixture(name);
    const api = vi.fn(async (input: RequestInfo | URL, _init?: RequestInit) =>
      String(input).endsWith("/launch-options")
        ? Response.json(emptyLaunchOptions("empty"))
        : new Response(JSON.stringify(body), {
          status: body.status,
          headers: { "content-type": "application/problem+json" },
        })
    );
    vi.stubGlobal("fetch", api);
    mount({
      ticketId: "T-720",
      ticketTitle: "Keep draft",
      initialInput: "Keep this request unchanged",
    });
    const start = await screen.findByRole("button", { name: "Start Worker" });
    await fireEvent.click(start);
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain(body.detail);
    expect(alert.querySelector("details")!.open).toBe(false);
    expect(alert.querySelector("details")!.textContent).not.toContain(
      body.detail,
    );
    expect(alert.textContent!.split(body.detail)).toHaveLength(2); // wrapper/title/detailの重複なし
    expect(
      (screen.getByLabelText("Initial request") as HTMLTextAreaElement).value,
    ).toBe("Keep this request unchanged");
    expect((start as HTMLButtonElement).disabled).toBe(name !== "rejected");
    if (name !== "rejected") {
      expect(screen.getByRole("link", { name: "Check Workers" })).toBeTruthy();
      expect(screen.getByRole("link", { name: "Check Ticket" })).toBeTruthy();
      await fireEvent.click(start);
      expect(api).toHaveBeenCalledTimes(2);
    }
    if (name === "compensation") {
      expect(screen.getByText("Backend cleanup: residual.")).toBeTruthy();
    }
  },
);

test.each(["invalid", "oversize", "unsupported", "unsafe-detail"])(
  "%s failure never renders raw bodies or silently allows a second launch",
  async (mode) => {
    const body = JSON.parse(JSON.stringify(startupFixture("rejected")));
    if (mode === "unsupported") body.yoi_startup.version = 99;
    if (mode === "unsafe-detail") {
      body.yoi_failure.details.raw_body = "/private/secret token=credential";
    }
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) =>
        String(input).endsWith("/launch-options")
          ? Response.json(emptyLaunchOptions("empty"))
          : new Response(
            mode === "invalid"
              ? "/private/secret raw body"
              : mode === "oversize"
              ? "secret".repeat(6000)
              : JSON.stringify(body),
            {
              status: 409,
              headers: { "content-type": "application/problem+json" },
            },
          )
      ),
    );
    mount();
    const start = await screen.findByRole("button", { name: "Start Worker" });
    await fireEvent.click(start);
    await screen.findByRole("alert");
    expect(document.body.textContent).not.toContain("/private/");
    expect(document.body.textContent).not.toContain("secret");
    expect(document.body.textContent).not.toContain("token=credential");
    expect((start as HTMLButtonElement).disabled).toBe(true);
  },
);

test.each([false, true])(
  "a created Worker never permits another POST even when navigation fails: %s",
  async (navigationFails) => {
    if (navigationFails) {
      vi.mocked(goto).mockRejectedValueOnce(new Error("navigation failed"));
    }
    const api = vi.fn(async (input: RequestInfo | URL) =>
      String(input).endsWith("/launch-options")
        ? Response.json(emptyLaunchOptions("empty"))
        : Response.json({
          workspace_id: "empty",
          runtime_id: "embedded",
          worker_id: "worker-a",
          console_href: "/w/empty/workers/W-1/console",
          diagnostics: [],
          worker: {
            runtime_id: "embedded",
            worker_id: "worker-a",
            host_id: "host-a",
            display_name: "Worker",
            label: "worker-a",
            profile: null,
            singleton_key: null,
            tags: [],
            workspace: {
              visibility: "workspace",
              identity: "empty",
              workspace_id: "empty",
            },
            state: "idle",
            last_seen_at: null,
            pinned: false,
            retention_state: "active",
            implementation: { kind: "runtime", display_hint: "Runtime Worker" },
            diagnostics: [],
          },
        })
    );
    vi.stubGlobal("fetch", api);
    mount();
    const start = await screen.findByRole("button", { name: "Start Worker" });
    await fireEvent.click(start);
    await waitFor(() =>
      expect(goto).toHaveBeenCalledWith("/w/empty/workers/W-1/console")
    );
    expect((start as HTMLButtonElement).disabled).toBe(true);
    if (navigationFails) {
      expect((await screen.findByRole("alert")).textContent).toContain(
        "The Worker was created, but its console could not be opened.",
      );
      expect(screen.getByRole("link", { name: "Check Workers" })).toBeTruthy();
    }
    await fireEvent.click(start);
    expect(api).toHaveBeenCalledTimes(2);
  },
);
