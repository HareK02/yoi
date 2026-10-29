// @vitest-environment happy-dom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import SubjectPermissions from "./SubjectPermissions.svelte";
import type { Policy, PolicyUpdate } from "./permissions.ts";
const ops = [
  {
    operation: "ticket.create",
    label: "Create Tickets",
    description: "Create an open Ticket in this Workspace.",
    resource_kinds: ["workspace"],
    availability: { kind: "available" },
  },
  {
    operation: "filesystem.read",
    label: "Read files",
    description: "Read only the selected Workdir path.",
    resource_kinds: ["workdir"],
    availability: { kind: "available" },
  },
  {
    operation: "filesystem.write",
    label: "Edit files",
    description: "Write only the selected Workdir path.",
    resource_kinds: ["workdir"],
    availability: { kind: "available" },
  },
  {
    operation: "worker.messaging",
    label: "Message Worker",
    description: "Separate operation use and directional target control.",
    resource_kinds: ["workspace"],
    availability: { kind: "unavailable", reason: "T-803 connection pending" },
  },
];
const props = { workspaceId: "w", subjectId: "s" };
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  sessionStorage.clear();
});
function harness(initial: Policy | null = null, canManage = true) {
  let policy = initial;
  const requests: PolicyUpdate[] = [];
  const fetch = vi.fn(async (_path: string, init?: RequestInit) => {
    if (init?.method === "POST") {
      const request = JSON.parse(init.body as string) as PolicyUpdate;
      requests.push(request);
      policy = {
        change_id: request.operation_id,
        execution: request.execution,
        delegation: request.delegation,
      };
      return Response.json({ permissions: policy, interruptions: [] });
    }
    return Response.json({
      permissions: policy,
      can_manage: canManage,
      operations: ops,
    });
  });
  vi.stubGlobal("fetch", fetch);
  return { fetch, requests, setPolicy: (next: Policy | null) => policy = next };
}
async function choose(name = "ticket.create") {
  await screen.findByRole("combobox", { name: "Operation" });
  await fireEvent.change(screen.getByRole("combobox", { name: "Operation" }), {
    target: { value: name },
  });
}
async function add(which = "execution", name = "ticket.create") {
  await choose(name);
  await fireEvent.change(
    screen.getByRole("combobox", { name: "Permission scope" }),
    { target: { value: which } },
  );
  if (name.startsWith("filesystem")) {
    await fireEvent.input(
      screen.getByRole("textbox", { name: "Workdir identifier" }),
      { target: { value: "wd-only" } },
    );
    await fireEvent.input(
      screen.getByRole("textbox", { name: "Relative path" }),
      { target: { value: "src" } },
    );
  }
  await fireEvent.click(
    screen.getByRole("button", { name: "Add to unsaved changes" }),
  );
}
async function confirmSave() {
  await fireEvent.click(screen.getByRole("checkbox", { name: /I reviewed/ }));
  await fireEvent.click(
    screen.getByRole("button", { name: "Save permissions" }),
  );
}
test("new Subject selects direct and narrow delegation independently, previews exact diff and saves through existing endpoint", async () => {
  const h = harness();
  render(SubjectPermissions, props);
  await add();
  await add("delegation", "filesystem.write");
  expect(h.requests).toHaveLength(0);
  expect(
    screen.getByRole("region", { name: "Unsaved permission differences" })
      .textContent,
  ).toContain("Edit files");
  expect(
    (screen.getByRole("button", {
      name: "Save permissions",
    }) as HTMLButtonElement).disabled,
  ).toBe(true);
  await confirmSave();
  await screen.findByText("Saved and re-read from the current policy.");
  expect(h.requests).toHaveLength(1);
  expect(h.requests[0].expected_change_id).toBe(null);
  expect(h.requests[0].execution[0].operation).toBe("ticket.create");
  expect(h.requests[0].delegation[0]).toEqual({
    operation: "filesystem.write",
    resource: {
      kind: "workdir",
      working_directory_id: "wd-only",
      path: "src",
      recursive: false,
    },
  });
  expect(h.fetch.mock.calls[1][0]).toBe(
    "/api/w/w/subjektiv/subjects/s/permissions",
  );
});
test("unsupported legacy target is retained verbatim while an unrelated operation is added", async () => {
  const legacy = {
    operation: "worker.notify",
    resource: {
      kind: "worker" as const,
      runtime_id: "r",
      worker_id: "only-target",
    },
  };
  const h = harness({ change_id: "old", execution: [], delegation: [legacy] });
  render(SubjectPermissions, props);
  await add();
  await confirmSave();
  await screen.findByText("Saved and re-read from the current policy.");
  expect(h.requests[0].delegation).toEqual([legacy]);
  expect(screen.getByText(/Preserved, not editable/)).toBeTruthy();
});
test("unimplemented Worker operation has reason, no Worker-ID fields and cannot be added", async () => {
  harness();
  render(SubjectPermissions, props);
  await choose("worker.messaging");
  expect(screen.getByText(/Target unavailable: T-803/)).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Add to unsaved changes" }))
    .toBeNull();
  expect(screen.queryByRole("textbox", { name: /Worker identifier/ }))
    .toBeNull();
});
test("read-only projection retains current permission visibility without save or edit actions", async () => {
  harness({
    change_id: "p",
    execution: [{
      operation: "ticket.create",
      resource: { kind: "workspace", workspace_id: "w" },
    }],
    delegation: [],
  }, false);
  render(SubjectPermissions, props);
  await screen.findByText(/Read-only. The Backend/);
  expect(screen.queryByRole("combobox")).toBeNull();
  expect(screen.queryByRole("button", { name: /^Remove/ })).toBeNull();
  expect(
    within(screen.getByRole("region", { name: "Direct operations" })).getByText(
      "Create Tickets",
    ),
  ).toBeTruthy();
});
test("conflict retains unsaved diff and blocks editing until explicit reload, never rebases CAS silently", async () => {
  const h = harness();
  h.fetch.mockImplementation(async (_path, init) =>
    init?.method === "POST"
      ? new Response(null, { status: 409 })
      : Response.json({ permissions: null, can_manage: true, operations: ops })
  );
  render(SubjectPermissions, props);
  await add();
  await confirmSave();
  await screen.findByText(/Conflict: another save/);
  expect(screen.getByRole("region", { name: "Unsaved permission differences" }))
    .toBeTruthy();
  expect(screen.queryByRole("button", { name: "Save permissions" })).toBeNull();
  expect(sessionStorage.length).toBe(0);
});
test("lost response survives remount and exact replay confirms receipt without reviving later policy", async () => {
  const h = harness();
  let saved: PolicyUpdate | null = null;
  const later: Policy = {
    change_id: "later-shrink",
    execution: [],
    delegation: [],
  };
  let lost = false;
  h.fetch.mockImplementation(async (_path, init) => {
    if (init?.method === "POST") {
      const request = JSON.parse(init.body as string) as PolicyUpdate;
      if (!lost) {
        saved = request;
        lost = true;
        throw new Error("lost after commit");
      }
      expect(request).toEqual(saved);
      return Response.json({
        permissions: {
          change_id: request.operation_id,
          execution: request.execution,
          delegation: request.delegation,
        },
        interruptions: [],
      });
    }
    return Response.json({
      permissions: lost ? later : null,
      can_manage: true,
      operations: ops,
    });
  });
  const first = render(SubjectPermissions, props);
  await add();
  await confirmSave();
  await screen.findByText("Save outcome not yet confirmed.");
  first.unmount();
  render(SubjectPermissions, props);
  await waitFor(() =>
    expect(
      (screen.getByRole("button", {
        name: "Reconcile the same save",
      }) as HTMLButtonElement).disabled,
    ).toBe(false)
  );
  await fireEvent.click(
    screen.getByRole("button", { name: "Reconcile the same save" }),
  );
  await screen.findByText(/original save was confirmed, but a later change/);
  expect(
    within(screen.getByRole("region", { name: "Direct operations" }))
      .queryByText("Create Tickets"),
  ).toBeNull();
  expect(sessionStorage.length).toBe(0);
});
test("deleted Subject is refused with no success indication", async () => {
  const h = harness();
  h.fetch.mockImplementation(async () => new Response(null, { status: 404 }));
  render(SubjectPermissions, props);
  await screen.findByText(
    /Target unavailable. This Subject may have been deleted/,
  );
  expect(screen.queryByRole("combobox")).toBeNull();
  expect(h.requests).toHaveLength(0);
});
