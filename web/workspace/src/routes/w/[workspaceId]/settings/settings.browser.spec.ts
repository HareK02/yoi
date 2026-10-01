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
import { goto, invalidate } from "$app/navigation";
import Settings from "./+page.svelte";
import {
  deletionFixture,
  identityFixture,
  metadataFixture,
  settingsFixtureHandler,
} from "#lib/workspace/settings/identity.test-fixtures.ts";
vi.mock(
  "$app/navigation",
  () => ({ goto: vi.fn(async () => {}), invalidate: vi.fn(async () => {}) }),
);
vi.mock(
  "#lib/workspace/multiplexer.ts",
  () => ({ disposeWorkspaceMultiplexer: vi.fn() }),
);
vi.mock(
  "#lib/workspace/sidebar/worker-subscription.ts",
  () => ({ disposeWorkspaceWorkersStore: vi.fn() }),
);
type Data = ComponentProps<typeof Settings>["data"];
function data(id = "home-owner", owner = true): Data {
  return {
    workspace: {
      workspace_id: id,
      display_name: metadataFixture(id).display_name,
      permissions: { delete_workspace: owner },
    },
  } as Data;
}
function mount(value = data()) {
  return render(Settings, {
    props: {
      data: value,
      params: { workspaceId: value.workspace!.workspace_id },
    },
  });
}
function mockApi(
  override?: (request: Request) => Promise<Response | null> | Response | null,
) {
  const handler = settingsFixtureHandler();
  const mock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = new Request(
      new URL(String(input), "https://fixture.test"),
      init,
    );
    return await override?.(request) ?? await handler(request) ??
      new Response("Unexpected fixture request", { status: 404 });
  });
  vi.stubGlobal("fetch", mock);
  return mock;
}
async function openName() {
  await fireEvent.click(
    await screen.findByRole("button", { name: "Edit name" }),
  );
  return screen.getByLabelText("New display name") as HTMLInputElement;
}
async function openDeletion() {
  await screen.findByText("Active", { exact: true });
  await fireEvent.click(
    screen.getByText("Delete Workspace", { selector: "summary" }),
  );
  await fireEvent.click(
    screen.getByRole("button", { name: "Review deletion impact" }),
  );
  return await screen.findByLabelText(
    `Type ${metadataFixture().display_name} to confirm`,
  ) as HTMLInputElement;
}
function operation(state = "succeeded", operationId = "op-test") {
  return {
    operation_id: operationId,
    workspace_id: "home-owner",
    display_name: metadataFixture().display_name,
    state,
    resources: deletionFixture().resources,
    child_operation_ids: [],
    blockers: [],
    failure_category: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    completed_at: null,
  };
}
afterEach(() => {
  cleanup();
  sessionStorage.clear();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

test("saved settings have no editable form, badges or raw data expanded; each technical scope is disclosed", async () => {
  const api = mockApi();
  mount();
  await screen.findByText("Active", { exact: true });
  expect(screen.getByText(metadataFixture().display_name)).toBeTruthy();
  expect(document.querySelector("form")).toBeNull();
  expect(document.querySelector("input, textarea:not([readonly])")).toBeNull();
  // happy-dom does not implement native details visibility; Chromium verifies it.
  expect(document.querySelector("textarea")?.closest("details")?.open).toBe(
    false,
  );
  expect(document.querySelectorAll("details[open]")).toHaveLength(0);
  expect(document.querySelector(".badge")).toBeNull();
  expect(api.mock.calls).toHaveLength(2);
  expect(api.mock.calls.some(([path]) => String(path).includes("deletion")))
    .toBe(false);
});

test("Edit focuses the field; Cancel discards the draft and returns focus without mutation", async () => {
  const api = mockApi();
  mount();
  const input = await openName();
  expect(document.activeElement).toBe(input);
  await fireEvent.input(input, { target: { value: "Discard this" } });
  await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(document.activeElement).toBe(
    screen.getByRole("button", { name: "Edit name" }),
  );
  expect((await openName()).value).toBe(metadataFixture().display_name);
  expect(api.mock.calls.every(([, init]) => !init?.method)).toBe(true);
});

test("save uses the current revision, publishes confirmed name and invalidates the Workspace header", async () => {
  const api = mockApi();
  mount();
  await fireEvent.input(await openName(), {
    target: { value: "Renamed Workspace" },
  });
  await fireEvent.click(screen.getByRole("button", { name: "Save name" }));
  await screen.findByText("Workspace name saved.");
  expect(screen.getByText("Renamed Workspace")).toBeTruthy();
  expect(document.querySelector("form")).toBeNull();
  expect(document.activeElement).toBe(
    screen.getByRole("button", { name: "Edit name" }),
  );
  expect(
    JSON.parse(String(
      api.mock.calls.find(([, init]) => init?.method === "PUT")![1]!.body,
    )),
  ).toEqual({
    display_name: "Renamed Workspace",
    revision: metadataFixture().revision,
  });
  expect(invalidate).toHaveBeenCalledWith("/api/w/home-owner/workspace");
});

test("failed save preserves the draft and saved name; Reload saved name recovers from revision conflicts", async () => {
  mockApi((request) =>
    request.method === "PUT"
      ? new Response("Revision conflict", { status: 409 })
      : null
  );
  mount();
  const input = await openName();
  await fireEvent.input(input, { target: { value: "Keep draft" } });
  await fireEvent.click(screen.getByRole("button", { name: "Save name" }));
  expect((await screen.findByRole("alert")).textContent).toBe(
    "Revision conflict",
  );
  expect(input.value).toBe("Keep draft");
  expect(screen.getByText(`Current: ${metadataFixture().display_name}`))
    .toBeTruthy();
  await fireEvent.click(
    screen.getByRole("button", { name: "Reload saved name" }),
  );
  await screen.findByRole("button", { name: "Edit name" });
  expect(screen.queryByRole("alert")).toBeNull();
  expect((await openName()).value).toBe(metadataFixture().display_name);
});

test("pending save disables duplicate submit and cancel while preserving the current value", async () => {
  let release!: (response: Response) => void;
  const held = new Promise<Response>((resolve) => release = resolve);
  const api = mockApi((request) => request.method === "PUT" ? held : null);
  mount();
  await fireEvent.input(await openName(), { target: { value: "New name" } });
  await fireEvent.click(screen.getByRole("button", { name: "Save name" }));
  expect(
    (screen.getByRole("button", { name: "Saving…" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  expect(
    (screen.getByRole("button", { name: "Cancel" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  await fireEvent.submit(document.querySelector("form")!);
  expect(api.mock.calls.filter(([, init]) => init?.method === "PUT"))
    .toHaveLength(1);
  release(
    Response.json({
      workspace: { ...metadataFixture(), display_name: "New name" },
      diagnostics: [],
    }),
  );
  await screen.findByText("Workspace name saved.");
});

test("metadata and public identity load independently and offer scoped retries", async () => {
  let failName = true;
  mockApi((request) =>
    failName && new URL(request.url).pathname.endsWith("/settings")
      ? new Response("Name unavailable", { status: 503 })
      : null
  );
  mount();
  await screen.findByText("Active", { exact: true });
  await screen.findByText("Name unavailable");
  expect(screen.queryByRole("button", { name: "Edit name" })).toBeNull();
  failName = false;
  await fireEvent.click(screen.getByRole("button", { name: "Retry name" }));
  await screen.findByRole("button", { name: "Edit name" });
});

test("identity failure does not block name editing or masquerade as pending provisioning", async () => {
  let fail = true;
  mockApi((request) =>
    fail && request.url.endsWith("/signing-identity")
      ? new Response("Identity unavailable", { status: 503 })
      : null
  );
  mount();
  await screen.findByRole("button", { name: "Edit name" });
  await screen.findByText("Identity unavailable");
  expect(screen.queryByRole("button", { name: "Provision identity" }))
    .toBeNull();
  fail = false;
  await fireEvent.click(screen.getByRole("button", { name: "Retry identity" }));
  await screen.findByText("Active", { exact: true });
});

test("non-owner retains metadata editing but never fetches or shows owner-only identity/deletion", async () => {
  const api = mockApi();
  mount(data("home-member", false));
  await screen.findByRole("button", { name: "Edit name" });
  expect(screen.queryByRole("heading", { name: "Public identity" })).toBeNull();
  expect(screen.queryByText("Delete Workspace")).toBeNull();
  expect(api).toHaveBeenCalledTimes(1);
});

test("pending provisioning is explicit, only provisions on action and focuses the resulting copy action", async () => {
  const api = mockApi();
  mount(data("home-empty"));
  await screen.findByText("Not provisioned");
  expect(api.mock.calls.every(([, init]) => !init?.method)).toBe(true);
  await fireEvent.click(
    screen.getByRole("button", { name: "Provision identity" }),
  );
  const copy = await screen.findByRole("button", { name: "Copy bundle" });
  expect(document.activeElement).toBe(copy);
  expect(screen.getByText("Active", { exact: true })).toBeTruthy();
});

test("copy uses the exact public bundle; clipboard failure offers manual selection rather than losing data", async () => {
  mockApi();
  const writeText = vi.fn().mockResolvedValueOnce(undefined)
    .mockRejectedValueOnce(new Error("Denied"));
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText },
  });
  mount();
  const button = await screen.findByRole("button", { name: "Copy bundle" });
  await fireEvent.click(button);
  await screen.findByText("Public bundle copied.");
  expect(writeText).toHaveBeenCalledWith(
    JSON.stringify(identityFixture().public_bundle, null, 2),
  );
  await fireEvent.click(button);
  expect((await screen.findByRole("alert")).textContent).toContain(
    "select the bundle manually",
  );
  expect(document.querySelector("textarea")!.value).toBe(
    JSON.stringify(identityFixture().public_bundle, null, 2),
  );
});

test("deletion is deferred, requires exact current name, and Cancel restores disclosure focus", async () => {
  const api = mockApi();
  mount();
  const input = await openDeletion();
  expect(document.activeElement).toBe(input);
  const confirm = screen.getByRole("button", {
    name: "Delete Workspace",
  }) as HTMLButtonElement;
  expect(confirm.disabled).toBe(true);
  await fireEvent.input(input, { target: { value: "Wrong name" } });
  expect(confirm.disabled).toBe(true);
  await fireEvent.input(input, {
    target: { value: metadataFixture().display_name },
  });
  expect(confirm.disabled).toBe(false);
  await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(document.activeElement?.textContent).toBe("Delete Workspace");
  expect(api.mock.calls.every(([, init]) => !init?.method)).toBe(true);
});

test("deletion blockers and preflight failure do not permit a destructive request", async () => {
  let fail = true;
  const api = mockApi((request) =>
    request.url.endsWith("/deletion")
      ? fail
        ? new Response("Preflight unavailable", { status: 503 })
        : Response.json({
          ...deletionFixture(),
          can_delete: false,
          blockers: [{
            kind: "worker_removal_blocked",
            resource_kind: "worker",
            resource_key: "W-7",
            message: "Worker W-7 is running.",
          }],
        })
      : null
  );
  mount();
  await screen.findByText("Active", { exact: true });
  await fireEvent.click(
    screen.getByText("Delete Workspace", { selector: "summary" }),
  );
  await fireEvent.click(
    screen.getByRole("button", { name: "Review deletion impact" }),
  );
  await screen.findByRole("button", { name: "Retry deletion check" });
  expect(screen.queryByRole("button", { name: "Delete Workspace" })).toBeNull();
  fail = false;
  await fireEvent.click(
    screen.getByRole("button", { name: "Retry deletion check" }),
  );
  await screen.findByText("Worker W-7 is running.");
  expect(
    (screen.getByRole("button", {
      name: "Delete Workspace",
    }) as HTMLButtonElement).disabled,
  ).toBe(true);
  expect(api.mock.calls.every(([, init]) => !init?.method)).toBe(true);
});

test("deletion retry reuses the persisted operation id and confirmation, then redirects only after success", async () => {
  let attempts = 0;
  const api = mockApi(async (request) => {
    if (request.method === "POST" && request.url.endsWith("/deletion")) {
      const body = await request.json();
      return ++attempts === 1
        ? new Response("Connection lost", { status: 503 })
        : Response.json(operation("queued", body.operation_id));
    }
    if (request.url.includes("/workspace-deletions/")) {
      return Response.json(
        operation("succeeded", request.url.split("/").pop()),
      );
    }
    return null;
  });
  mount();
  await fireEvent.input(await openDeletion(), {
    target: { value: metadataFixture().display_name },
  });
  await fireEvent.click(
    screen.getByRole("button", { name: "Delete Workspace" }),
  );
  await screen.findByRole("button", { name: "Retry deletion" });
  expect(goto).not.toHaveBeenCalled();
  const stored = sessionStorage.getItem("yoi:workspace-deletion:home-owner");
  expect(stored).toBeTruthy();
  await fireEvent.click(screen.getByRole("button", { name: "Retry deletion" }));
  await waitFor(() => expect(goto).toHaveBeenCalledWith("/"));
  const requests = api.mock.calls.filter(([, init]) => init?.method === "POST");
  expect(requests[0][1]!.body).toBe(requests[1][1]!.body);
  expect(sessionStorage.getItem("yoi:workspace-deletion:home-owner"))
    .toBeNull();
});

test("reload resumes persisted deletion without sending another POST", async () => {
  sessionStorage.setItem(
    "yoi:workspace-deletion:home-owner",
    JSON.stringify({
      operation_id: "op-test",
      expected_revision: metadataFixture().revision,
      confirmation: metadataFixture().display_name,
    }),
  );
  const api = mockApi((request) =>
    request.url.includes("/workspace-deletions/")
      ? Response.json(operation("failed"))
      : null
  );
  mount();
  await screen.findByText("Deletion state: failed");
  expect(document.querySelector("details.deletion")?.hasAttribute("open")).toBe(
    true,
  );
  expect(api.mock.calls.every(([, init]) => !init?.method)).toBe(true);
});

test("Workspace switch resets drafts and ignores old load/mutation completions", async () => {
  let release!: (response: Response) => void;
  const held = new Promise<Response>((resolve) => release = resolve);
  mockApi((request) => request.method === "PUT" ? held : null);
  const view = mount();
  await fireEvent.input(await openName(), {
    target: { value: "Old scope draft" },
  });
  await fireEvent.click(screen.getByRole("button", { name: "Save name" }));
  await view.rerender({
    data: data("home-long"),
    params: { workspaceId: "home-long" },
  });
  await screen.findByText(metadataFixture("home-long").display_name);
  release(
    Response.json({
      workspace: { ...metadataFixture(), display_name: "Old scope draft" },
      diagnostics: [],
    }),
  );
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(screen.queryByText("Old scope draft")).toBeNull();
  expect(invalidate).not.toHaveBeenCalled();
  expect(goto).not.toHaveBeenCalled();
});

test("slow identity keeps the name usable and old identity responses cannot leak across Workspaces", async () => {
  let release!: (response: Response) => void;
  const held = new Promise<Response>((resolve) => release = resolve);
  mockApi((request) =>
    request.url.endsWith("/home-owner/settings/signing-identity") ? held : null
  );
  const view = mount();
  await screen.findByRole("button", { name: "Edit name" });
  expect(screen.getByText("Loading identity…")).toBeTruthy();
  await view.rerender({
    data: data("home-empty"),
    params: { workspaceId: "home-empty" },
  });
  await screen.findByText("Not provisioned");
  release(Response.json(identityFixture()));
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(screen.queryByText("Active", { exact: true })).toBeNull();
  expect(screen.getByText("Not provisioned")).toBeTruthy();
});

test("a completed deletion request from an unmounted Workspace cannot navigate away from the new scope", async () => {
  const request = {
    operation_id: "op-test",
    expected_revision: metadataFixture().revision,
    confirmation: metadataFixture().display_name,
  };
  sessionStorage.setItem(
    "yoi:workspace-deletion:home-owner",
    JSON.stringify(request),
  );
  let release!: (response: Response) => void;
  const held = new Promise<Response>((resolve) => release = resolve);
  mockApi((request) =>
    request.url.includes("/workspace-deletions/") ? held : null
  );
  const view = mount();
  await screen.findByText("Deleting…");
  await view.rerender({
    data: data("home-member", false),
    params: { workspaceId: "home-member" },
  });
  await screen.findByRole("button", { name: "Edit name" });
  release(Response.json(operation()));
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(goto).not.toHaveBeenCalled();
  expect(sessionStorage.getItem("yoi:workspace-deletion:home-owner")).not
    .toBeNull();
});

test("name editing and deletion confirmation cannot overlap", async () => {
  mockApi();
  mount();
  await openName();
  await fireEvent.click(
    screen.getByText("Delete Workspace", { selector: "summary" }),
  );
  expect(
    (screen.getByRole("button", {
      name: "Review deletion impact",
    }) as HTMLButtonElement).disabled,
  ).toBe(true);
  await fireEvent.click(
    screen.getByRole("button", { name: "Cancel" }),
  );
  await fireEvent.click(
    screen.getByRole("button", { name: "Review deletion impact" }),
  );
  await screen.findByLabelText(
    `Type ${metadataFixture().display_name} to confirm`,
  );
  expect(
    (screen.getByRole("button", { name: "Edit name" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  await fireEvent.click(
    screen.getByRole("button", { name: "Cancel" }),
  );
  expect(
    (screen.getByRole("button", { name: "Edit name" }) as HTMLButtonElement)
      .disabled,
  ).toBe(false);
});

test("untrusted names and errors remain text, not interpreted HTML", async () => {
  const name = "<img src=x onerror=alert(1)>";
  mockApi((request) =>
    request.url.endsWith("/settings")
      ? Response.json({ ...metadataFixture(), display_name: name })
      : request.url.endsWith("/signing-identity")
      ? new Response("<script>bad()</script>", { status: 503 })
      : null
  );
  mount();
  await screen.findByText(name);
  expect((await screen.findByRole("alert")).textContent).toBe(
    "<script>bad()</script>",
  );
  expect(document.querySelector("img, script")).toBeNull();
});
