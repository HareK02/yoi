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
import { identityFixture } from "#lib/workspace/settings/identity.test-fixtures.ts";

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

test("Runtime registration requires and preserves the Runtime-issued enrollment ID independently of the fingerprint", async () => {
  api.fingerprint.mockResolvedValue("sha256:runtime-public-key");
  api.create.mockResolvedValue({});
  const data = {
    workspaceId: "workspace-a",
    workspace: { permissions: { manage_runtimes: true } },
    runtimes: { items: [], diagnostics: [] },
    signingIdentity: identityFixture("workspace-a"),
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
  expect(submit).toHaveProperty("disabled", true);
  expect(api.create).not.toHaveBeenCalled();
  await fireEvent.input(screen.getByLabelText("Workspace trust ID"), {
    target: { value: "Runtime-issued-enrollment-A" },
  });
  expect(submit).toHaveProperty("disabled", false);
  await fireEvent.submit(submit.closest("form")!);
  await waitFor(() => expect(api.create).toHaveBeenCalledOnce());
  expect(api.create).toHaveBeenCalledWith("workspace-a", {
    public_bundle: {
      identity_id: "runtime-a",
      public_key: "runtime-public-key",
    },
    workspace_trust_id: "Runtime-issued-enrollment-A",
    display_name: null,
    endpoint: "https://runtime.example",
    expected_binding_id: null,
  });
});

test("Runtime reactivation submits a new enrollment ID with the observed binding instead of reusing revoked trust", async () => {
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
            state: "revoked",
            connection_state: "revoked",
            binding_id: "observed-binding",
            workspace_trust_id: "revoked-enrollment",
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
  expect(screen.getByLabelText("Workspace trust ID")).toHaveProperty(
    "value",
    "",
  );
  await fireEvent.input(screen.getByLabelText("Workspace trust ID"), {
    target: { value: "new-enrollment" },
  });
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
    workspace_trust_id: "new-enrollment",
    display_name: "Runtime A",
    endpoint: "https://runtime.example",
    expected_binding_id: "observed-binding",
  });
});
