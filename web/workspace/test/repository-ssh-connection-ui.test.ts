import { assert, assertEquals } from "jsr:@std/assert";
import type { WorkspaceRuntimeResource } from "../src/lib/generated/runtime-api.ts";
import { parseRepositorySshConnectionProbeResponse } from "../src/lib/workspace/api/workspace-model.ts";
import {
  changeRepositorySshProbeRuntime,
  RepositorySshProbeFence,
  repositorySshProbeRuntimes,
} from "../src/lib/workspace/repositories/ssh-connection.ts";

const root = new URL("../", import.meta.url);
const pageSource = await Deno.readTextFile(
  new URL(
    "./src/routes/w/[workspaceId]/repositories/[repositoryKey]/+page.svelte",
    root,
  ),
);
const loaderSource = await Deno.readTextFile(
  new URL(
    "./src/routes/w/[workspaceId]/repositories/[repositoryKey]/+page.ts",
    root,
  ),
);

Deno.test("Repository SSH probe parser preserves the confirmation contract", () => {
  const response = {
    workspace_id: "workspace-a",
    repository_key: "main",
    runtime_id: "runtime-a",
    hostname: "example.test",
    port: 22,
    trust_state: "untrusted" as const,
    host_trust_id: "tofu-example.test-22",
    expected_host_trust_revision: null,
    candidates: [
      {
        algorithm: "ssh-ed25519",
        host_key: "ssh-ed25519 AAAA",
        fingerprint: "SHA256:host",
      },
    ],
  };

  assertEquals(parseRepositorySshConnectionProbeResponse(response), response);
});

Deno.test("Repository SSH probe offers configured remote Runtimes regardless of worker-style status", () => {
  const configured = {
    runtime_id: "arcadia",
    label: "Arcadia",
    kind: "remote_worker_runtime",
    status: "idle",
    diagnostics: [],
    management: {
      endpoint_configured: true,
      endpoint_display: "https://arcadia.example",
      binding: {
        state: "verified",
      },
    },
  } as unknown as WorkspaceRuntimeResource;
  const embedded = {
    ...configured,
    runtime_id: "embedded-worker-runtime",
    kind: "embedded_worker_runtime",
  } as unknown as WorkspaceRuntimeResource;
  const revoked = {
    ...configured,
    runtime_id: "revoked",
    management: {
      ...configured.management,
      binding: { state: "revoked" },
    },
  } as unknown as WorkspaceRuntimeResource;
  const unbound = {
    ...configured,
    runtime_id: "unbound",
    management: {
      ...configured.management,
      binding: undefined,
    },
  } as unknown as WorkspaceRuntimeResource;

  assertEquals(
    repositorySshProbeRuntimes([embedded, configured, revoked, unbound]).map((
      runtime,
    ) => runtime.runtime_id),
    ["arcadia"],
  );
});

Deno.test("changing the SSH probe Runtime clears the prior result and confirmation key", () => {
  const runtimeAProbe = {
    runtime_id: "runtime-a",
    candidates: [{ host_key: "ssh-ed25519 runtime-a" }],
  };

  assertEquals(
    changeRepositorySshProbeRuntime(
      "runtime-a",
      "runtime-b",
      runtimeAProbe,
      "ssh-ed25519 runtime-a",
    ),
    {
      changed: true,
      runtimeId: "runtime-b",
      probe: null,
      selectedHostKey: "",
    },
  );
});

Deno.test("a delayed SSH probe response cannot apply after the Runtime changes", async () => {
  const fence = new RepositorySshProbeFence();
  const operation = fence.capture("runtime-a");
  let renderedRuntimeId: string | null = null;
  let resolveProbe!: (runtimeId: string) => void;
  const delayedProbe = new Promise<string>((resolve) => {
    resolveProbe = resolve;
  }).then((runtimeId) => {
    if (fence.isCurrent(operation, "runtime-b")) {
      renderedRuntimeId = runtimeId;
    }
  });

  fence.enter("runtime-b");
  resolveProbe("runtime-a");
  await delayedProbe;
  assertEquals(renderedRuntimeId, null);
});

Deno.test("Repository SSH connection test requires an explicit host-key confirmation", () => {
  for (
    const token of [
      "Check SSH connection",
      "selectedRuntimeId",
      "candidate.fingerprint",
      "Confirm and trust selected host key",
      "expected_host_trust_revision",
      "requestConnectionTest('POST'",
      "requestConnectionTest('PUT'",
    ]
  ) {
    assert(pageSource.includes(token), `missing SSH connection flow ${token}`);
  }
});

Deno.test("Repository detail loads configured Workspace Runtimes for the connection test", () => {
  assert(loaderSource.includes('workspaceApiPath(workspaceId, "/runtimes")'));
  assert(loaderSource.includes("parseWorkspaceRuntimeList"));
});
