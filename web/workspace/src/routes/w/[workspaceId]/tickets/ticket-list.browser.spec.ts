// @vitest-environment happy-dom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/svelte";
import { type ComponentProps, tick } from "svelte";
import { afterEach, expect, test, vi } from "vitest";
import type {
  TicketBrowseQueryItem,
  TicketBrowseQueryResponse,
} from "#lib/generated/ticket-api.ts";
import { loadJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
import { loadWorkspaceRepositoryList } from "#lib/workspace/api/repositories.ts";
import { parseBrowserWorkspaceOrchestratorResponse } from "#lib/workspace/api/workers.ts";
import { compileTicketSearch } from "#lib/workspace/tickets/ticket-search.ts";
import TicketsPage from "./PageContent.svelte";
import { load } from "./+page.ts";

vi.mock("$app/navigation", () => ({ goto: vi.fn(async () => {}) }));
type Data = ComponentProps<typeof TicketsPage>["data"];

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
function ticket(id = "1", title = "First ticket"): TicketBrowseQueryItem {
  return {
    id,
    resource_key: `T-${id}`,
    title,
    state: "open",
    updated_at: null,
    tags: [],
    current_closure: null,
    assignment_tracking: [],
  };
}
function browse(
  items = [ticket()],
  cursor: string | null = null,
): TicketBrowseQueryResponse {
  return {
    items,
    record_authority: "sqlite",
    page: {
      has_more: cursor !== null,
      next_cursor: cursor,
      limit: 50,
      returned: items.length,
      sort: "updated_desc",
      source_limit: null,
      source_truncated: false,
    },
  };
}
function data(
  workspaceId = "workspace-a",
  result = browse(),
  search = "state:open",
): Data {
  return {
    workspaceId,
    search,
    queryRequest: compileTicketSearch(search, []),
    tickets: { data: result, error: null },
  } as Data;
}
function status(workspaceId = "workspace-a", online = true) {
  return {
    workspace_id: workspaceId,
    online,
    disposition: "observed",
    worker: null,
    diagnostics: [],
  };
}
function repositories(workspaceId = "workspace-a") {
  return {
    workspace_id: workspaceId,
    items: [],
    source: "sqlite",
    diagnostics: [],
  };
}
function requests(routes: Record<string, () => Promise<Response>>) {
  const request = vi.fn(
    (input: RequestInfo | URL, _init?: RequestInit): Promise<Response> => {
      const route = routes[String(input)];
      if (!route) throw new Error(`Unexpected request: ${input}`);
      return route();
    },
  );
  vi.stubGlobal("fetch", request);
  return request;
}
async function loadPage(
  fetchFn: typeof fetch,
  workspaceId = "workspace-a",
  search = "state:open",
) {
  const result = await load(
    {
      parent: async () => ({}),
      fetch: fetchFn,
      params: { workspaceId },
      url: new URL(
        `https://example.test/w/${workspaceId}/tickets?q=${
          encodeURIComponent(search)
        }`,
      ),
    } as Parameters<typeof load>[0],
  ).deferredPage;
  if (!result.ok) throw new Error(result.failure.message);
  return result.data as Data;
}
// Let response body decoding and Svelte effects finish before negative stale-result assertions.
async function flushResponses() {
  await new Promise((resolve) => setTimeout(resolve, 0));
  await tick();
}
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

test("actual loader posts browse only; rows render while orchestrator is unresolved and repositories stay lazy", async () => {
  const rows = deferred<Response>();
  const orchestrator = deferred<Response>();
  const repos = deferred<Response>();
  const request = requests({
    "/api/w/workspace-a/tickets/browse": () => rows.promise,
    "/api/w/workspace-a/orchestrator": () => orchestrator.promise,
    "/api/w/workspace-a/repositories": () => repos.promise,
  });
  const pending = loadPage(request);
  await waitFor(() => expect(request).toHaveBeenCalledTimes(1));
  expect(request.mock.calls[0][0]).toBe("/api/w/workspace-a/tickets/browse");
  expect(request.mock.calls[0][1]?.method).toBe("POST");
  expect(JSON.parse(String(request.mock.calls[0][1]?.body))).toEqual(
    compileTicketSearch("state:open", []),
  );
  rows.resolve(Response.json(browse()));
  const view = render(TicketsPage, { data: await pending });
  expect(await screen.findByText("First ticket")).toBeTruthy();
  expect(screen.getByText("Checking…")).toBeTruthy();
  expect(
    view.container.querySelector(".orchestrator-status")?.getAttribute(
      "aria-busy",
    ),
  ).toBe("true");
  expect(screen.queryByText("Offline")).toBeNull();
  expect(screen.queryByRole("button", { name: "Start Orchestrator" }))
    .toBeNull();
  expect(request.mock.calls.map(([path]) => path)).toEqual([
    "/api/w/workspace-a/tickets/browse",
    "/api/w/workspace-a/orchestrator",
  ]);
  await fireEvent.click(screen.getByRole("button", { name: "Add Ticket" }));
  expect(
    request.mock.calls.filter(([path]) =>
      String(path).endsWith("/repositories")
    ),
  ).toHaveLength(1);
  expect(screen.getByText("Loading repositories…")).toBeTruthy();
  expect(screen.getByText("First ticket")).toBeTruthy();
  repos.resolve(Response.json(repositories()));
  orchestrator.resolve(Response.json(status()));
  expect(await screen.findByText("Online")).toBeTruthy();
});

test("lazy repository HTTP failure is visible and retry enables targetless creation without reloading tickets", async () => {
  const first = deferred<Response>();
  const retry = deferred<Response>();
  const repoRequest = vi.fn().mockReturnValueOnce(first.promise)
    .mockReturnValueOnce(retry.promise);
  const request = requests({
    "/api/w/workspace-a/orchestrator": async () => Response.json(status()),
    "/api/w/workspace-a/repositories": repoRequest,
  });
  render(TicketsPage, { data: data() });
  expect(repoRequest).not.toHaveBeenCalled();
  await fireEvent.click(screen.getByRole("button", { name: "Add Ticket" }));
  await fireEvent.input(screen.getByLabelText("Title"), {
    target: { value: "New title" },
  });
  expect(
    (screen.getByRole("button", { name: "Create Ticket" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  first.resolve(new Response("private failure body", { status: 503 }));
  expect(await screen.findByRole("alert")).toHaveProperty(
    "textContent",
    "Repositories: Repository API request failed with HTTP 503",
  );
  expect(screen.queryByText(/private failure body/)).toBeNull();
  await fireEvent.click(
    screen.getByRole("button", { name: "Retry repositories" }),
  );
  expect(repoRequest).toHaveBeenCalledTimes(2);
  expect(screen.getByText("Loading repositories…")).toBeTruthy();
  retry.resolve(Response.json(repositories()));
  await waitFor(() =>
    expect(
      (screen.getByRole("button", {
        name: "Create Ticket",
      }) as HTMLButtonElement).disabled,
    ).toBe(false)
  );
  expect(screen.queryByRole("alert")).toBeNull();
  expect(screen.getByText("First ticket")).toBeTruthy();
  await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  await fireEvent.click(screen.getByRole("button", { name: "Add Ticket" }));
  expect(repoRequest).toHaveBeenCalledTimes(2);
  expect(request.mock.calls.some(([path]) => String(path).includes("/tickets")))
    .toBe(false);
});

test("failed status is Unavailable, not Offline, and does not hide rows", async () => {
  const orchestrator = deferred<Response>();
  requests({ "/api/w/workspace-a/orchestrator": () => orchestrator.promise });
  render(TicketsPage, { data: data() });
  expect(screen.getByText("Checking…")).toBeTruthy();
  orchestrator.reject(new Error("Controlled status failure"));
  expect(await screen.findByText("Unavailable")).toBeTruthy();
  expect(screen.getByText("Orchestrator status: Controlled status failure"))
    .toBeTruthy();
  expect(screen.queryByText("Offline")).toBeNull();
  expect(screen.queryByRole("button", { name: "Start Orchestrator" }))
    .toBeNull();
  expect(screen.getByText("First ticket")).toBeTruthy();
});

test("late prior-Workspace status and repository responses cannot replace current state or finish its pending load", async () => {
  const oldStatus = deferred<Response>();
  const oldRepos = deferred<Response>();
  const newStatus = deferred<Response>();
  const newRepos = deferred<Response>();
  requests({
    "/api/w/workspace-a/orchestrator": () => oldStatus.promise,
    "/api/w/workspace-a/repositories": () => oldRepos.promise,
    "/api/w/workspace-b/orchestrator": () => newStatus.promise,
    "/api/w/workspace-b/repositories": () => newRepos.promise,
  });
  const view = render(TicketsPage, { data: data() });
  await fireEvent.click(screen.getByRole("button", { name: "Add Ticket" }));
  await view.rerender({
    data: data(
      "workspace-b",
      browse([ticket("2", "Current Workspace ticket")]),
    ),
  });
  expect(screen.queryByText("New Ticket")).toBeNull();
  expect(screen.queryByText("First ticket")).toBeNull();
  await fireEvent.click(screen.getByRole("button", { name: "Add Ticket" }));
  await fireEvent.input(screen.getByLabelText("Title"), {
    target: { value: "Current draft" },
  });
  oldStatus.resolve(Response.json(status("workspace-a", false)));
  oldRepos.resolve(Response.json(repositories("workspace-a")));
  await flushResponses();
  expect(screen.getByText("Checking…")).toBeTruthy();
  expect(screen.queryByText("Offline")).toBeNull();
  expect(screen.getByText("Loading repositories…")).toBeTruthy();
  expect(
    (screen.getByRole("button", { name: "Create Ticket" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  expect(screen.getByText("Current Workspace ticket")).toBeTruthy();
  newStatus.resolve(Response.json(status("workspace-b")));
  newRepos.resolve(Response.json(repositories("workspace-b")));
  expect(await screen.findByText("Online")).toBeTruthy();
  await waitFor(() =>
    expect(
      (screen.getByRole("button", {
        name: "Create Ticket",
      }) as HTMLButtonElement).disabled,
    ).toBe(false)
  );
  expect((screen.getByLabelText("Title") as HTMLInputElement).value).toBe(
    "Current draft",
  );
});

test("pagination posts browse with the query and cursor, preserves server order, and merges duplicate ids", async () => {
  const next = deferred<Response>();
  const request = requests({
    "/api/w/workspace-a/orchestrator": async () => Response.json(status()),
    "/api/w/workspace-a/tickets/browse": () => next.promise,
  });
  const initial = data("workspace-a", browse([ticket()], "cursor-1"));
  const view = render(TicketsPage, { data: initial });
  await fireEvent.click(
    screen.getByRole("button", { name: "Load more tickets" }),
  );
  const call = request.mock.calls.find(([path]) =>
    String(path).endsWith("/tickets/browse")
  )!;
  expect(call[1]?.method).toBe("POST");
  expect(JSON.parse(String(call[1]?.body))).toEqual({
    ...initial.queryRequest,
    cursor: "cursor-1",
  });
  expect(screen.getByText("First ticket")).toBeTruthy();
  next.resolve(
    Response.json(
      browse([
        ticket("1", "Updated first ticket"),
        ticket("2", "Second ticket"),
      ]),
    ),
  );
  expect(await screen.findByText("2 tickets loaded")).toBeTruthy();
  expect(
    [...view.container.querySelectorAll(".ticket-list-title")].map((node) =>
      node.textContent
    ),
  ).toEqual(["Updated first ticket", "Second ticket"]);
  expect(screen.queryByRole("button", { name: "Load more tickets" }))
    .toBeNull();
});

for (const staleFailure of [false, true]) {
  test(`prior-query pagination ${staleFailure ? "failure" : "success"} cannot overwrite current rows, cursor, or busy state`, async () => {
    const oldPage = deferred<Response>();
    const newPage = deferred<Response>();
    const pageRequest = vi.fn().mockReturnValueOnce(oldPage.promise)
      .mockReturnValueOnce(newPage.promise);
    const request = requests({
      "/api/w/workspace-a/orchestrator": async () => Response.json(status()),
      "/api/w/workspace-a/tickets/browse": pageRequest,
    });
    const view = render(TicketsPage, {
      data: data("workspace-a", browse([ticket()], "old-cursor")),
    });
    await fireEvent.click(
      screen.getByRole("button", { name: "Load more tickets" }),
    );
    const current = data(
      "workspace-a",
      browse([ticket("2", "Current query ticket")], "new-cursor"),
      "state:closed",
    );
    await view.rerender({ data: current });
    await fireEvent.click(
      screen.getByRole("button", { name: "Load more tickets" }),
    );
    oldPage.resolve(
      staleFailure
        ? new Response("stale error", { status: 500 })
        : Response.json(
          browse([ticket("3", "Stale page ticket")], "stale-cursor"),
        ),
    );
    await flushResponses();
    expect(screen.queryByText("Stale page ticket")).toBeNull();
    expect(screen.queryByText("First ticket")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByText("Current query ticket")).toBeTruthy();
    expect(screen.getByText("Loading…")).toBeTruthy();
    expect(
      screen.getByRole("region", { name: "Tickets" }).getAttribute("aria-busy"),
    ).toBe("true");
    const calls = request.mock.calls.filter(([path]) =>
      String(path).endsWith("/tickets/browse")
    );
    expect(JSON.parse(String(calls[1][1]?.body))).toEqual({
      ...current.queryRequest,
      cursor: "new-cursor",
    });
    newPage.resolve(Response.json(browse([ticket("4", "Current next page")])));
    expect(await screen.findByText("Current next page")).toBeTruthy();
    expect(screen.getByText("2 tickets loaded")).toBeTruthy();
    expect(screen.queryByText("Loading…")).toBeNull();
  });
}

test("controlled loader-to-component timing isolates the former blocking Promise.all dependency gate", async () => {
  // A local dependency-gating comparison, NOT a production latency benchmark.
  // Both arms use the actual current browse loader/parser and real Svelte component.
  // The baseline restores only the old Promise.all wait for status + repositories;
  // it deliberately does not emulate the old enriched query's backend work.
  const delays = { tickets: 20, secondary: 100 };
  const measurements: Record<string, number> = {};
  for (const blocking of [true, false]) {
    const outstanding: Promise<Response>[] = [];
    const delayed = (payload: unknown, ms: number) => {
      const promise = new Promise<Response>((resolve) =>
        setTimeout(() => resolve(Response.json(payload)), ms)
      );
      outstanding.push(promise);
      return promise;
    };
    const request = requests({
      "/api/w/workspace-a/tickets/browse": () =>
        delayed(browse(), delays.tickets),
      "/api/w/workspace-a/orchestrator": () =>
        delayed(status(), delays.secondary),
      "/api/w/workspace-a/repositories": () =>
        delayed(repositories(), delays.secondary),
    });
    const start = performance.now();
    const page = blocking
      ? (await Promise.all([
        loadPage(request),
        loadJson(
          request,
          workspaceApiPath("workspace-a", "/orchestrator"),
          undefined,
          parseBrowserWorkspaceOrchestratorResponse,
        ),
        loadWorkspaceRepositoryList(request, "workspace-a"),
      ]))[0]
      : await loadPage(request);
    render(TicketsPage, { data: page });
    expect(await screen.findByText("First ticket")).toBeTruthy();
    measurements[blocking ? "blockingBaselineMs" : "actualLoaderMs"] = Number(
      (performance.now() - start).toFixed(2),
    );
    if (!blocking) {
      expect(
        request.mock.calls.some(([path]) =>
          String(path).endsWith("/repositories")
        ),
      ).toBe(false);
    }
    cleanup();
    await Promise.all(outstanding);
    await flushResponses();
  }
  // No speed threshold: scheduler noise must not turn this into a flaky performance gate.
  console.info(
    "Controlled mocked loader-to-component timing (not production):",
    JSON.stringify({ delaysMs: delays, ...measurements }),
  );
});
