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
import { permissionSource } from "../../../../test/permissions-fixture.ts";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});
const props = { workspaceId: "w", subjectId: "s" };
function harness(source = permissionSource(), status = 200) {
  const fetcher = vi.fn(async (_input: string, _init?: RequestInit) =>
    new Response(JSON.stringify(source), { status })
  );
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
test("Subject source view separates Set/Namespace, direct/delegation and server origins; no legacy resource editor", async () => {
  const fetcher = harness();
  const view = await ready();
  expect(
    view.getByRole("link", { name: "Edit configuration sources" }).getAttribute(
      "href",
    ),
  ).toBe("/w/w/settings/configuration");
  const execution = within(
    view.getByRole("region", { name: "Direct operations" }),
  );
  const delegation = within(
    view.getByRole("region", { name: "Delegation ceiling" }),
  );
  expect(execution.getByText("Set · companion")).not.toBeNull();
  expect(execution.getByText("Namespace · ticket · ticket.hoge.fuga.finish"))
    .not.toBeNull();
  expect(delegation.getByText("Revoked")).not.toBeNull();
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
  expect(fetcher).toHaveBeenCalledExactlyOnceWith("/api/w/w/config/adoption");
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
test("migration draft cannot be displayed as an adopted effective policy", async () => {
  const source = permissionSource();
  source.source_stored = false;
  source.history_head = null;
  harness(source);
  const view = await ready();
  expect(view.getByText("Migration draft · not adopted")).not.toBeNull();
  expect(view.getAllByText("Draft operation key resolution")).toHaveLength(2);
  expect(view.queryByText("Adopted configuration source")).toBeNull();
});
test("empty and missing source assignments never restore defaults", async () => {
  const source = permissionSource();
  source.policies = [];
  harness(source);
  const view = await ready();
  expect(view.container.textContent).toContain(
    "No default or effective permission is inferred",
  );
  expect(view.queryByRole("region", { name: "Direct operations" })).toBeNull();
});
test.each([401, 403])(
  "owner rejection %s shows neither edit link nor invented policy",
  async (status) => {
    harness(permissionSource(), status);
    const view = render(SubjectPermissions, props);
    await waitFor(() =>
      expect(view.getByRole("alert").textContent).toContain(
        "Workspace owner only",
      )
    );
    expect(view.queryByRole("link", { name: "Edit configuration sources" }))
      .toBeNull();
  },
);
test("failed refresh removes previous authority and requires explicit reload", async () => {
  const fetcher = harness();
  const view = await ready();
  fetcher.mockResolvedValueOnce(new Response("Denied", { status: 403 }));
  await fireEvent.click(
    view.getByRole("button", { name: "Reload permissions" }),
  );
  await waitFor(() =>
    expect(view.getByRole("alert").textContent).toContain(
      "Workspace owner only",
    )
  );
  expect(view.queryByRole("region", { name: "Direct operations" })).toBeNull();
  expect(view.queryByRole("link", { name: "Edit configuration sources" }))
    .toBeNull();
});
test.each(["execution_resolution", "delegation_resolution"] as const)(
  "missing %s is an invalid source snapshot, not a separate resolver error or empty allow",
  async (field) => {
    const fetcher = harness();
    const source = permissionSource();
    const { [field]: _missing, ...policy } = source.policies[0];
    fetcher.mockResolvedValueOnce(
      new Response(JSON.stringify({ ...source, policies: [policy] })),
    );
    const view = render(SubjectPermissions, props);
    await waitFor(() =>
      expect(view.getByRole("alert").textContent).toContain(
        "No effective permission is inferred",
      )
    );
    expect(view.queryByRole("region", { name: "Direct operations" }))
      .toBeNull();
    expect(view.queryByText("Permission origin")).toBeNull();
    expect(view.queryByRole("link", { name: "Edit configuration sources" }))
      .toBeNull();
    expect(fetcher).toHaveBeenCalledExactlyOnceWith("/api/w/w/config/adoption");
  },
);
test("source load stays pending without showing an empty or previous policy", async () => {
  let complete!: (response: Response) => void;
  const fetcher = vi.fn(() =>
    new Promise<Response>((resolve) => {
      complete = resolve;
    })
  );
  vi.stubGlobal("fetch", fetcher);
  const view = render(SubjectPermissions, props);
  expect(view.getByRole("status").textContent).toContain(
    "Reading configuration source and permissions",
  );
  expect(view.queryByRole("region", { name: "Direct operations" })).toBeNull();
  expect(view.queryByText("No allowed operation keys.")).toBeNull();
  complete(new Response(JSON.stringify(permissionSource())));
  await waitFor(() =>
    expect(view.getByRole("region", { name: "Direct operations" })).not
      .toBeNull()
  );
  expect(fetcher).toHaveBeenCalledOnce();
});
