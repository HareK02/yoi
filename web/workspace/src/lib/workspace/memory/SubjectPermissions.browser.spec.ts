// @vitest-environment happy-dom
import {
  cleanup,
  fireEvent,
  render,
  waitFor,
  within,
} from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import SubjectPermissions from "./SubjectPermissions.svelte";
import {
  permissionPolicyResponse,
  permissionSource,
} from "../../../../test/permissions-fixture.ts";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});
const props = { workspaceId: "w", subjectId: "s" };
const policyPath = "/api/w/w/subjektiv/subjects/s/permission-policy";
function harness(source: unknown = permissionPolicyResponse(), status = 200) {
  const fetcher = vi.fn(async (input: string, init?: RequestInit) => {
    expect(input).toBe(policyPath);
    expect(init?.method ?? "GET").toBe("GET");
    return new Response(JSON.stringify(source), { status });
  });
  vi.stubGlobal("fetch", fetcher);
  return fetcher;
}
async function ready() {
  const view = render(SubjectPermissions, props);
  await waitFor(() =>
    expect(view.container.querySelector('[data-permission-state="ready"]')).not
      .toBeNull()
  );
  return view;
}
test("current policy separates Set/Namespace, direct/delegation and origins without a configuration editor", async () => {
  const fetcher = harness();
  const view = await ready();
  expect(view.queryByRole("link", { name: "Edit configuration sources" }))
    .toBeNull();
  const execution = within(
    view.getByRole("region", { name: "Direct operations" }),
  );
  const delegation = within(
    view.getByRole("region", { name: "Delegation ceiling" }),
  );
  expect(execution.getByText("Set · companion")).not.toBeNull();
  expect(execution.getByText("Namespace · ticket · ticket.hoge.fuga.finish"))
    .not.toBeNull();
  expect(delegation.getByText("Revoked assignment")).not.toBeNull();
  expect(delegation.getByText("No allowed operation keys.")).not.toBeNull();
  expect(
    execution.getByText("ticket.hoge.fuga.finish", {
      selector: ".resolved > li > code",
    }),
  ).not.toBeNull();
  await fireEvent.click(execution.getByText("Permission origin"));
  expect(execution.getByText("builtin:ticket")).not.toBeNull();
  expect(execution.getByText("ticket.hoge.fuga", { selector: "span > code" }))
    .not.toBeNull();
  expect(view.container.querySelectorAll("input, select, textarea"))
    .toHaveLength(0);
  expect(view.queryByRole("button", { name: /Save|Add|Remove/ })).toBeNull();
  expect(fetcher).toHaveBeenCalledExactlyOnceWith(policyPath);
  await fireEvent.click(view.getByText("Retained domain restrictions"));
  expect(view.container.querySelector("pre")?.textContent).toContain(
    '"ticket_id": "T-837"',
  );
  await fireEvent.click(view.getByText("Feature declaration catalog"));
  expect(view.container.textContent).toContain(
    "Explicit children: ticket.hoge.fuga",
  );
  expect(view.container.textContent).toContain("Same prefix, not included");
  expect(view.container.textContent).toContain("Disabled");
});
test.each([false, true])(
  "configuration snapshot source_stored=%s cannot become current policy",
  async (stored) => {
    const source = permissionSource();
    source.source_stored = stored;
    source.history_head = stored ? "head:adopted" : null;
    harness(source);
    const view = render(SubjectPermissions, props);
    await waitFor(() =>
      expect(view.getByRole("alert").textContent).toContain(
        "No effective permission is inferred",
      )
    );
    expect(view.queryByRole("region", { name: "Direct operations" }))
      .toBeNull();
    expect(view.queryByText("Permission origin")).toBeNull();
  },
);
test.each([null, "policy:removed"])(
  "unset policy with change_id=%s never restores defaults",
  async (change_id) => {
    harness({
      ...permissionPolicyResponse(),
      policy: null,
      change_id,
      execution_resolution: {
        workspace: "w",
        allowed: {},
        diagnostics: [{ kind: "unset" }],
      },
      delegation_resolution: {
        workspace: "w",
        allowed: {},
        diagnostics: [{ kind: "unset" }],
      },
    });
    const view = await ready();
    expect(view.container.textContent).toContain(
      "No default assignment or effective permission is inferred",
    );
    expect(view.getAllByText("Unset assignment")).toHaveLength(2);
    expect(view.getAllByText("No allowed operation keys.")).toHaveLength(2);
    expect(view.queryByText("Permission origin")).toBeNull();
    expect(view.getByText(change_id ?? "No policy change recorded")).not
      .toBeNull();
    if (change_id) {
      expect(view.container.textContent).toContain(
        "removal tombstone",
      );
    }
  },
);
test("explicitly empty assignments remain distinct from unset and revoked", async () => {
  const response = permissionPolicyResponse();
  for (const lane of ["execution", "delegation"] as const) {
    response.policy[lane] = { grants: [], exclusions: [], revoked: false };
  }
  for (
    const lane of ["execution_resolution", "delegation_resolution"] as const
  ) response[lane] = { workspace: "w", allowed: {}, diagnostics: [] };
  harness(response);
  const view = await ready();
  expect(view.getAllByText("Explicitly empty assignment")).toHaveLength(2);
  expect(view.getAllByText("No allowed operation keys.")).toHaveLength(2);
  expect(view.queryByText("Unset assignment")).toBeNull();
  expect(view.queryByText("Revoked assignment")).toBeNull();
});
test.each(
  [
    [401, "Sign in to read the current Subject policy."],
    [403, "You do not have permission to read the current Subject policy."],
  ] as const,
)(
  "HTTP %s shows its failure without inventing policy",
  async (status, message) => {
    harness(permissionPolicyResponse(), status);
    const view = render(SubjectPermissions, props);
    await waitFor(() =>
      expect(view.getByRole("alert").textContent).toContain(message)
    );
    expect(view.queryByRole("region", { name: "Direct operations" }))
      .toBeNull();
    expect(view.queryByRole("link", { name: "Edit configuration sources" }))
      .toBeNull();
  },
);
test("failed refresh removes previous authority and permits explicit reload", async () => {
  const fetcher = harness();
  const view = await ready();
  fetcher.mockResolvedValueOnce(new Response("Denied", { status: 403 }));
  await fireEvent.click(
    view.getByRole("button", { name: "Reload permissions" }),
  );
  await waitFor(() =>
    expect(view.getByRole("alert").textContent).toContain(
      "You do not have permission",
    )
  );
  expect(view.queryByRole("region", { name: "Direct operations" })).toBeNull();
  expect(view.queryByRole("link", { name: "Edit configuration sources" }))
    .toBeNull();
  expect(fetcher).toHaveBeenCalledTimes(2);
  await fireEvent.click(
    view.getByRole("button", { name: "Reload permissions" }),
  );
  await waitFor(() =>
    expect(view.getByRole("region", { name: "Direct operations" })).not
      .toBeNull()
  );
  expect(fetcher.mock.calls).toEqual([[policyPath], [policyPath], [
    policyPath,
  ]]);
});
test.each(["execution_resolution", "delegation_resolution"] as const)(
  "missing %s invalidates the dedicated response rather than becoming empty allow",
  async (field) => {
    const { [field]: _missing, ...source } = permissionPolicyResponse();
    const fetcher = harness(source);
    const view = render(SubjectPermissions, props);
    await waitFor(() =>
      expect(view.getByRole("alert").textContent).toContain(
        "No effective permission is inferred",
      )
    );
    expect(view.queryByRole("region", { name: "Direct operations" }))
      .toBeNull();
    expect(view.queryByText("Permission origin")).toBeNull();
    expect(fetcher).toHaveBeenCalledExactlyOnceWith(policyPath);
  },
);
test("policy load stays pending without showing an empty or previous policy", async () => {
  let complete!: (response: Response) => void;
  const fetcher = vi.fn(() =>
    new Promise<Response>((resolve) => {
      complete = resolve;
    })
  );
  vi.stubGlobal("fetch", fetcher);
  const view = render(SubjectPermissions, props);
  expect(view.getByRole("status").textContent).toContain(
    "Reading current Subject policy",
  );
  expect(view.queryByRole("region", { name: "Direct operations" })).toBeNull();
  expect(view.queryByText("No allowed operation keys.")).toBeNull();
  complete(new Response(JSON.stringify(permissionPolicyResponse())));
  await waitFor(() =>
    expect(view.getByRole("region", { name: "Direct operations" })).not
      .toBeNull()
  );
  expect(fetcher).toHaveBeenCalledExactlyOnceWith(policyPath);
});
