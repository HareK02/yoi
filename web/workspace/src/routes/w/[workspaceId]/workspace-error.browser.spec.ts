// @vitest-environment happy-dom
import { cleanup, fireEvent, render } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import WorkspaceError from "./+error.svelte";
import { load } from "./+layout.ts";

const state = vi.hoisted(() => ({
  page: {
    params: { workspaceId: "workspace-a" },
    status: 500,
    error: { message: "Ticket Browser API response is invalid" },
  },
}));
vi.mock("$app/state", () => state);
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

test("a child page error preserves its message and never claims Workspace deletion", async () => {
  const reload = vi.spyOn(window.location, "reload").mockImplementation(
    () => {},
  );
  const view = render(WorkspaceError);
  expect(view.getByRole("alert").textContent).toBe(
    "Ticket Browser API response is invalid",
  );
  expect(view.getByText("Error 500")).toBeTruthy();
  expect(view.queryByText("The selected Workspace cannot be opened"))
    .toBeNull();
  expect(view.queryByText("Workspace unavailable")).toBeNull();
  expect(view.queryByText("Create Workspace")).toBeNull();
  expect(view.queryByText("Choose another Workspace")).toBeNull();
  await fireEvent.click(view.getByRole("button", { name: "Reload page" }));
  expect(reload).toHaveBeenCalledOnce();
});

async function loadFailure(workspaceFetch: () => Promise<Response>) {
  const fetcher = vi.fn(async (input: RequestInfo | URL) => {
    if (String(input).endsWith("/workspace")) return await workspaceFetch();
    return new Response("repository lookup unavailable", { status: 503 });
  });
  return await (await load(
    {
      fetch: fetcher,
      parent: async () => ({}),
      params: { workspaceId: "workspace-a" },
    } as unknown as Parameters<typeof load>[0],
  )).deferredWorkspace;
}

test.each([401, 403, 404, 500, 503])(
  "Workspace HTTP %i is not rewritten as 404",
  async (status) => {
    const result = await loadFailure(async () =>
      new Response("private backend body", { status })
    );
    expect(result.ok).toBe(false);
    if (result.ok) throw new Error("expected a failed Workspace load");
    expect(result.failure.status).toBe(status);
    expect(result.failure.message).toBe(
      `Workspace API request failed with HTTP ${status}`,
    );
    expect(result.failure.message).not.toContain("private backend body");
  },
);

test.each(["network", "json", "contract"] as const)(
  "Workspace %s failures do not imply a missing Workspace",
  async (kind) => {
    const result = await loadFailure(async () => {
      if (kind === "network") throw new TypeError("Failed to fetch");
      return kind === "json" ? new Response("not json") : Response.json({});
    });
    expect(result.ok).toBe(false);
    if (result.ok) throw new Error("expected a failed Workspace load");
    expect(result.failure.status).toBeNull();
    expect(result.failure.message).toBe(
      {
        network: "Workspace API request could not be completed",
        json: "Workspace API response could not be decoded as JSON",
        contract: "Workspace API response is invalid",
      }[kind],
    );
    expect(result.failure.message).not.toContain("removed");
    expect(result.failure.message).not.toContain("404");
  },
);
