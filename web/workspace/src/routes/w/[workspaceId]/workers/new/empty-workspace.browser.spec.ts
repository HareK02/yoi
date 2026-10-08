// @vitest-environment happy-dom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import type { ComponentProps } from "svelte";
import NewWorker from "./+page.svelte";
function mount() {
  const data = {
    workspaceId: "empty",
    repositories: {
      workspace_id: "empty",
      items: [],
      source: "fixture",
      diagnostics: [],
    },
    ticketContext: null,
  } as unknown as ComponentProps<typeof NewWorker>["data"];
  return render(NewWorker, {
    props: { data, params: { workspaceId: "empty" } },
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
    profile: "builtin:companion",
    workdir_attachments: [],
    ticket_assignment: null,
  });
  expect(body).not.toHaveProperty("repository_key");
});

test("zero-repository Workspace does not bypass a remote Runtime Workdir requirement", async () => {
  const options = emptyLaunchOptions("empty");
  vi.stubGlobal(
    "fetch",
    vi.fn(async () =>
      Response.json({ ...options, runtimes: [...options.runtimes].reverse() })
    ),
  );
  mount();
  await screen.findByRole("button", { name: "Start Worker" });
  await screen.findByLabelText("Attachment 1 Workdir");
  expect(
    (screen.getByRole("button", { name: "Start Worker" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
});
