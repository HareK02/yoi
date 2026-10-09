// @vitest-environment happy-dom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import { tick } from "svelte";
import WorkerDriveGrants from "./WorkerDriveGrants.svelte";
import {
  createDriveGrant,
  listDriveGrants,
  parseDriveGrantListResponse,
} from "./api.ts";
import type { DriveGrantResponse } from "#lib/generated/drive-api.ts";

const workers = [
  {
    runtime_id: "runtime-a",
    worker_id: "worker",
    display_name: "Alpha",
    label: "Alpha",
  },
  {
    runtime_id: "runtime-b",
    worker_id: "worker",
    display_name: "Beta",
    label: "Beta",
  },
];
function grant(
  overrides: Partial<DriveGrantResponse> = {},
): DriveGrantResponse {
  return {
    grant_id: "1",
    workspace_id: "space",
    runtime_id: "runtime-a",
    worker_id: "worker",
    access: "read_only",
    revoked: false,
    created_by: "owner",
    created_at: "now",
    revoked_by: null,
    revoked_at: null,
    ...overrides,
  };
}
const page = (
  grants: DriveGrantResponse[] = [],
  next_after: string | null = null,
) => Response.json({ grants, next_after });
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}
async function select(index = 0) {
  const combo = screen.getByRole("combobox", {
    name: "Worker",
  }) as HTMLSelectElement;
  await fireEvent.change(combo, {
    target: {
      value: JSON.stringify([
        workers[index].runtime_id,
        workers[index].worker_id,
      ]),
    },
  });
  expect(combo.value).toBe(
    JSON.stringify([workers[index].runtime_id, workers[index].worker_id]),
  );
}
const props = { workspaceId: "space", workers, canManage: true };
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

test("non-owner has no grant surface and makes no grant requests", async () => {
  const fetch = vi.fn();
  vi.stubGlobal("fetch", fetch);
  render(WorkerDriveGrants, { ...props, canManage: false });
  await tick();
  expect(screen.queryByRole("region", { name: "Worker Drive grants" }))
    .toBeNull();
  expect(fetch).not.toHaveBeenCalled();
});

test("catalog readiness gates selection and removed Workers cannot retain grants", async () => {
  const fetch = vi.fn(async () => page([grant()]));
  vi.stubGlobal("fetch", fetch);
  const view = render(WorkerDriveGrants, { ...props, workersReady: false });
  expect(screen.queryByRole("combobox")).toBeNull();
  expect(fetch).not.toHaveBeenCalled();
  await view.rerender({ workersReady: true });
  await select();
  expect(await screen.findByRole("button", { name: "Revoke Drive grant 1" }))
    .toBeTruthy();
  await view.rerender({ workers: [workers[1]] });
  expect(screen.queryByRole("button", { name: /Revoke/ })).toBeNull();
  expect(
    (screen.getByRole("combobox", { name: "Worker" }) as HTMLSelectElement)
      .value,
  ).toBe("");
});

test("all pages are loaded and active grants match both runtime and Worker identities", async () => {
  const second = deferred<Response>();
  const fetch = vi.fn().mockResolvedValueOnce(
    page([grant({ runtime_id: "runtime-b" })], "1"),
  ).mockReturnValueOnce(second.promise);
  vi.stubGlobal("fetch", fetch);
  render(WorkerDriveGrants, props);
  await select();
  await waitFor(() => expect(fetch).toHaveBeenCalledTimes(2));
  expect(fetch.mock.calls[1][0]).toBe(
    "/api/w/space/drive/grants?limit=200&after=1",
  );
  expect(screen.queryByText("No active Drive grants for this Worker."))
    .toBeNull();
  second.resolve(
    page([
      grant({ grant_id: "2" }),
      grant({ grant_id: "3", worker_id: "another" }),
      grant({ grant_id: "4", revoked: true }),
    ]),
  );
  expect(await screen.findByRole("button", { name: "Revoke Drive grant 2" }))
    .toBeTruthy();
  expect(screen.getAllByRole("button", { name: /Revoke/ })).toHaveLength(1);
  expect(screen.queryByRole("button", { name: "Add Drive grant" })).toBeNull();
});

test.each(["read_only", "read_write"] as const)(
  "create %s and revoke use current Backend authentication and confirmed state",
  async (access) => {
    const created = grant({ access });
    const fetch = vi.fn().mockResolvedValueOnce(page()).mockResolvedValueOnce(
      Response.json(created),
    ).mockResolvedValueOnce(page([created])).mockResolvedValueOnce(
      Response.json({
        ...created,
        revoked: true,
        revoked_at: "now",
        revoked_by: "owner",
      }),
    ).mockResolvedValueOnce(page());
    vi.stubGlobal("fetch", fetch);
    render(WorkerDriveGrants, props);
    await select();
    await fireEvent.click(
      await screen.findByRole("button", { name: "Add Drive grant" }),
    );
    await fireEvent.change(screen.getByRole("combobox", { name: "Access" }), {
      target: { value: access },
    });
    await fireEvent.click(
      screen.getByRole("button", { name: "Create Drive grant" }),
    );
    const revoke = await screen.findByRole("button", {
      name: "Revoke Drive grant 1",
    });
    await waitFor(() =>
      expect((revoke as HTMLButtonElement).disabled).toBe(false)
    );
    expect(JSON.parse(fetch.mock.calls[1][1].body)).toEqual({
      runtime_id: "runtime-a",
      worker_id: "worker",
      access,
    });
    expect(fetch.mock.calls[1][1].credentials).toBe("same-origin");
    expect(fetch.mock.calls[1][1].headers).toEqual({
      "content-type": "application/json",
    });
    await fireEvent.click(revoke);
    await waitFor(() =>
      expect(screen.queryByRole("button", { name: /Revoke/ })).toBeNull()
    );
    expect(fetch.mock.calls[3][0]).toBe("/api/w/space/drive/grants/1");
    expect(fetch.mock.calls[3][1].method).toBe("DELETE");
  },
);

test.each(["typed unknown", "transport loss", "malformed success"])(
  "%s blocks mutation retry and offers only an explicit authority refresh",
  async (kind) => {
    const fetch = vi.fn().mockResolvedValueOnce(page());
    if (kind === "transport loss") {
      fetch.mockRejectedValueOnce(new Error("lost"));
    } else {fetch.mockResolvedValueOnce(
        kind === "typed unknown"
          ? Response.json({
            code: "outcome_unknown",
            classification: "unknown",
            message: "unknown",
          }, { status: 503 })
          : Response.json({}),
      );}
    fetch.mockResolvedValueOnce(page([grant()]));
    vi.stubGlobal("fetch", fetch);
    render(WorkerDriveGrants, props);
    await select();
    await fireEvent.click(
      await screen.findByRole("button", { name: "Add Drive grant" }),
    );
    await fireEvent.click(
      screen.getByRole("button", { name: "Create Drive grant" }),
    );
    expect((await screen.findByRole("alert")).textContent).toContain(
      "outcome unknown",
    );
    expect(screen.queryByRole("button", { name: "Create Drive grant" }))
      .toBeNull();
    expect(screen.queryByText("No active Drive grants for this Worker."))
      .toBeNull();
    expect(fetch).toHaveBeenCalledTimes(2);
    await fireEvent.click(
      screen.getByRole("button", { name: "Refresh grants" }),
    );
    expect(await screen.findByRole("button", { name: "Revoke Drive grant 1" }))
      .toBeTruthy();
    expect(fetch.mock.calls.filter((call) => call[1]?.method === "POST"))
      .toHaveLength(1);
  },
);

test("typed denial is not reported as an unknown or successful mutation", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValueOnce(page()).mockResolvedValueOnce(
      Response.json({
        code: "denied",
        classification: "not_committed",
        message: "Drive access denied",
      }, { status: 403 }),
    ),
  );
  render(WorkerDriveGrants, props);
  await select();
  await fireEvent.click(
    await screen.findByRole("button", { name: "Add Drive grant" }),
  );
  await fireEvent.click(
    screen.getByRole("button", { name: "Create Drive grant" }),
  );
  expect((await screen.findByRole("alert")).textContent).toBe(
    "Drive access denied",
  );
  expect(screen.queryByText("Drive grant created.")).toBeNull();
});

test.each(["Worker", "Workspace", "permission", "removed Worker"])(
  "late mutation response cannot affect a changed %s scope",
  async (change) => {
    const pending = deferred<Response>();
    const fetch = vi.fn().mockResolvedValueOnce(page()).mockReturnValueOnce(
      pending.promise,
    ).mockResolvedValue(page());
    vi.stubGlobal("fetch", fetch);
    const view = render(WorkerDriveGrants, props);
    await select();
    await fireEvent.click(
      await screen.findByRole("button", { name: "Add Drive grant" }),
    );
    await fireEvent.click(
      screen.getByRole("button", { name: "Create Drive grant" }),
    );
    if (change === "Worker") await select(1);
    if (change === "Workspace") await view.rerender({ workspaceId: "other" });
    if (change === "permission") await view.rerender({ canManage: false });
    if (change === "removed Worker") {
      await view.rerender({
        workers: [workers[1]],
      });
    }
    pending.resolve(Response.json(grant()));
    await pending.promise;
    await tick();
    await tick();
    expect(screen.queryByText("Drive grant created.")).toBeNull();
    expect(screen.queryByRole("button", { name: "Revoke Drive grant 1" }))
      .toBeNull();
  },
);

test("late list response cannot replace the selected Worker state", async () => {
  const pending = deferred<Response>();
  vi.stubGlobal(
    "fetch",
    vi.fn().mockReturnValueOnce(pending.promise).mockResolvedValueOnce(
      page([grant({ runtime_id: "runtime-b", grant_id: "2" })]),
    ),
  );
  render(WorkerDriveGrants, props);
  await select();
  await select(1);
  expect(await screen.findByRole("button", { name: "Revoke Drive grant 2" }))
    .toBeTruthy();
  pending.resolve(page([grant()]));
  await pending.promise;
  await tick();
  await tick();
  expect(screen.queryByRole("button", { name: "Revoke Drive grant 1" }))
    .toBeNull();
  expect(screen.getByRole("button", { name: "Revoke Drive grant 2" }))
    .toBeTruthy();
});

test("unchanged catalog identities do not clear an in-flight mutation", async () => {
  const pending = deferred<Response>();
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValueOnce(page()).mockReturnValueOnce(pending.promise)
      .mockResolvedValueOnce(page([grant()])),
  );
  const view = render(WorkerDriveGrants, props);
  await select();
  await fireEvent.click(
    await screen.findByRole("button", { name: "Add Drive grant" }),
  );
  await fireEvent.click(
    screen.getByRole("button", { name: "Create Drive grant" }),
  );
  await view.rerender({
    workers: workers.map((worker) => ({
      ...worker,
      display_name: `${worker.label} updated`,
    })),
  });
  expect(
    (screen.getByRole("button", { name: "Saving…" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  pending.resolve(Response.json(grant()));
  expect(await screen.findByText("Drive grant created.")).toBeTruthy();
});

test("pending create accepts only one submission", async () => {
  const pending = deferred<Response>();
  const fetch = vi.fn().mockResolvedValueOnce(page()).mockReturnValueOnce(
    pending.promise,
  ).mockResolvedValueOnce(page([grant()]));
  vi.stubGlobal("fetch", fetch);
  render(WorkerDriveGrants, props);
  await select();
  await fireEvent.click(
    await screen.findByRole("button", { name: "Add Drive grant" }),
  );
  const button = screen.getByRole("button", { name: "Create Drive grant" });
  await fireEvent.click(button);
  await fireEvent.submit(button.closest("form")!);
  expect(fetch.mock.calls.filter((call) => call[1]?.method === "POST"))
    .toHaveLength(1);
  pending.resolve(Response.json(grant()));
  expect(await screen.findByText("Drive grant created.")).toBeTruthy();
});

test("an uncertain revoke cannot claim removal or permit a blind retry", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValueOnce(page([grant()])).mockRejectedValueOnce(
      new Error("transport lost"),
    ),
  );
  render(WorkerDriveGrants, props);
  await select();
  await fireEvent.click(
    await screen.findByRole("button", { name: "Revoke Drive grant 1" }),
  );
  expect((await screen.findByRole("alert")).textContent).toContain(
    "outcome unknown",
  );
  expect(screen.queryByText("Drive grant revoked.")).toBeNull();
  expect(screen.queryByRole("button", { name: /Revoke/ })).toBeNull();
});

test("complete Backend Worker identifiers are available in a disclosure", async () => {
  const item = {
    ...workers[0],
    runtime_id: `runtime-${"r".repeat(140)}`,
    worker_id: `worker-${"w".repeat(140)}`,
    display_name: "Long Worker name ".repeat(12),
  };
  vi.stubGlobal("fetch", vi.fn(async () => page()));
  render(WorkerDriveGrants, { ...props, workers: [item] });
  await fireEvent.change(screen.getByRole("combobox", { name: "Worker" }), {
    target: { value: JSON.stringify([item.runtime_id, item.worker_id]) },
  });
  await screen.findByRole("button", { name: "Add Drive grant" });
  const identity = screen.getByText("Worker identity").closest("details");
  expect(screen.getByText(item.runtime_id).closest("details")).toBe(identity);
  expect(screen.getByText(item.worker_id).closest("details")).toBe(identity);
});

test("parser rejects malformed access, missing revocation state and noncanonical grant cursors", () => {
  for (
    const value of [
      { grants: [grant({ access: "admin" as never })], next_after: null },
      { grants: [{ ...grant(), revoked: undefined }], next_after: null },
      { grants: [], next_after: "01" },
    ]
  ) expect(() => parseDriveGrantListResponse(value)).toThrow();
});

test("foreign Workspace pages and cyclic cursors never become a complete grant list", async () => {
  for (
    const result of [page([grant({ workspace_id: "other" })]), page([], "1")]
  ) {
    vi.stubGlobal("fetch", vi.fn(async () => result));
    await expect(listDriveGrants("space")).rejects.toThrow();
  }
});

test("successful response with wrong Worker identity is an unknown mutation outcome", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => Response.json(grant({ runtime_id: "runtime-b" }))),
  );
  await expect(
    createDriveGrant("space", {
      runtime_id: "runtime-a",
      worker_id: "worker",
      access: "read_only",
    }),
  ).rejects.toMatchObject({ outcome: "unknown" });
});

test("Catalog readiness refresh does not unlock or abandon a pending grant on the same Worker", async () => {
  const pending = deferred<Response>();
  let committed = false;
  const fetch = vi.fn((_: RequestInfo | URL, init?: RequestInit) => {
    if (init?.method === "POST") return pending.promise;
    return Promise.resolve(page(committed ? [grant()] : []));
  });
  vi.stubGlobal("fetch", fetch);
  const view = render(WorkerDriveGrants, { ...props, workersReady: true });
  await select();
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "Add Drive grant" }))
      .toBeDefined()
  );
  await fireEvent.click(
    screen.getByRole("button", { name: "Add Drive grant" }),
  );
  await fireEvent.click(
    screen.getByRole("button", { name: "Create Drive grant" }),
  );
  await view.rerender({ ...props, workersReady: false });
  await view.rerender({ ...props, workersReady: true });
  await tick();
  const add = screen.queryByRole("button", { name: "Add Drive grant" }) as
    | HTMLButtonElement
    | null;
  if (add) expect(add.disabled).toBe(true);
  const saving = screen.getByRole("button", {
    name: "Saving…",
  }) as HTMLButtonElement;
  expect(saving.disabled).toBe(true);
  committed = true;
  pending.resolve(Response.json(grant()));
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "Revoke Drive grant 1" }))
      .toBeDefined()
  );
  expect(fetch.mock.calls.filter(([, init]) => init?.method === "POST"))
    .toHaveLength(1);
});
