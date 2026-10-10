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
import RuntimeSettings from "./+page.svelte";
import RuntimeDetail from "./[runtimeId]/+page.svelte";

const api = vi.hoisted(() => ({ create: vi.fn(), fingerprint: vi.fn() }));
vi.mock("$app/navigation", () => ({ invalidateAll: vi.fn(async () => {}) }));
vi.mock("#lib/workspace/api/runtime-management.ts", async (importOriginal) => ({
  ...await importOriginal<
    typeof import("#lib/workspace/api/runtime-management.ts")
  >(),
  createRemoteRuntime: api.create,
  previewRuntimePublicKeyFingerprint: api.fingerprint,
}));

afterEach(() => {
  cleanup();
  vi.resetAllMocks();
});

test("Runtime registration uses public bundle without requiring copied Workspace enrollment identity", async () => {
  api.fingerprint.mockResolvedValue("sha256:runtime-public-key");
  api.create.mockResolvedValue({});
  const data = {
    workspaceId: "workspace-a",
    workspace: { permissions: { manage_runtimes: true } },
    runtimes: { items: [], diagnostics: [] },
    signingIdentity: null,
    signingIdentityError: "Workspace public identity could not be loaded",
  } as unknown as ComponentProps<typeof RuntimeSettings>["data"];
  render(RuntimeSettings, {
    props: { data, params: { workspaceId: "workspace-a" } },
  });
  await fireEvent.click(screen.getByRole("button", { name: "Add Runtime" }));
  await fireEvent.input(
    screen.getByLabelText("Runtime public bundle", { selector: "textarea" }),
    {
      target: {
        value: JSON.stringify({
          identity_id: "runtime-a",
          public_key: "runtime-public-key",
        }),
      },
    },
  );
  await fireEvent.input(screen.getByLabelText("Endpoint"), {
    target: { value: "https://runtime.example" },
  });
  await fireEvent.click(
    screen.getByRole("button", { name: "Preview fingerprint" }),
  );
  const submit = screen.getByRole("button", { name: "Register Runtime" });
  await waitFor(() =>
    expect(screen.getByText("sha256:runtime-public-key")).toBeTruthy()
  );
  expect(screen.queryByLabelText("Workspace trust ID")).toBeNull();
  expect(submit).toHaveProperty("disabled", false);
  await fireEvent.submit(submit.closest("form")!);
  await waitFor(() => expect(api.create).toHaveBeenCalledOnce());
  expect(api.create).toHaveBeenCalledWith("workspace-a", {
    public_bundle: {
      identity_id: "runtime-a",
      public_key: "runtime-public-key",
    },
    display_name: null,
    endpoint: "https://runtime.example",
    expected_binding_id: null,
  });
});

test("Runtime reactivation uses the observed binding without copied Workspace identity", async () => {
  api.fingerprint.mockResolvedValue("sha256:replacement-key");
  api.create.mockResolvedValue({});
  const data = {
    workspaceId: "workspace-a",
    runtimeId: "runtime-a",
    workspace: { permissions: { manage_runtimes: true } },
    runtimeDetail: {
      workspace_id: "workspace-a",
      endpoint: "https://runtime.example",
      runtime: {
        runtime_id: "runtime-a",
        label: "Runtime A",
        diagnostics: [],
        management: {
          built_in: false,
          config_managed: true,
          removable: true,
          binding: {
            revoked_at: "2026-10-10T00:00:00Z",
            binding_id: "observed-binding",
          },
        },
      },
      trust_key: {
        status: "revoked",
        fingerprint: "sha256:old-key",
        binding_id: "observed-binding",
      },
      recent_audit: [],
    },
  } as unknown as ComponentProps<typeof RuntimeDetail>["data"];
  render(RuntimeDetail, {
    props: {
      data,
      params: { workspaceId: "workspace-a", runtimeId: "runtime-a" },
    },
  });
  expect(screen.queryByLabelText("Workspace trust ID")).toBeNull();
  await fireEvent.input(screen.getByLabelText("Runtime public key"), {
    target: { value: "yoi-ed25519-pub:v1:new-key" },
  });
  await waitFor(() =>
    expect(screen.getByText("sha256:replacement-key")).toBeTruthy()
  );
  await fireEvent.submit(
    screen.getByRole("button", { name: "Reactivate with this key" }).closest(
      "form",
    )!,
  );
  await waitFor(() => expect(api.create).toHaveBeenCalledOnce());
  expect(api.create).toHaveBeenCalledWith("workspace-a", {
    public_bundle: {
      identity_id: "runtime-a",
      public_key: "yoi-ed25519-pub:v1:new-key",
    },
    display_name: "Runtime A",
    endpoint: "https://runtime.example",
    expected_binding_id: "observed-binding",
  });
});
