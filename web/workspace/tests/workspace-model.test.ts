declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
  readTextFile(path: URL): Promise<string>;
};

import {
  parseRepositoryListApiResult,
  parseRepositoryListResponse,
  parseWorkspaceResponse,
} from "../src/lib/workspace/api/workspace-model.ts";

function assertThrows(operation: () => unknown, expected: string): void {
  try {
    operation();
  } catch (error) {
    if (error instanceof Error && error.message.includes(expected)) return;
    throw error;
  }
  throw new Error("expected operation to throw");
}

const repositoryList = {
  workspace_id: "w-a",
  items: [{
    repository_key: "main",
    kind: "git",
    provider: "git",
    source: { kind: "local_path", uri: "/srv/alpha" },
    source_revision: 1,
    source_fingerprint: "sha256:alpha",
    observed_status: "ready",
    record_authority: "workspace-control-plane",
  }],
  source: "workspace-control-plane",
  diagnostics: [],
};

Deno.test("generated repository wrapper validates current Backend JSON", () => {
  const parsed = parseRepositoryListResponse(repositoryList);
  if (parsed.items[0]?.repository_key !== "main") {
    throw new Error("repository key was not preserved");
  }
  if (parsed.items[0]?.source.kind !== "local_path") {
    throw new Error("repository source kind was not preserved");
  }
});

Deno.test("stale repository aliases fail closed at the JSON boundary", () => {
  const stale = structuredClone(repositoryList) as Record<string, unknown>;
  const items = stale.items as Array<Record<string, unknown>>;
  items[0].id = items[0].repository_key;
  delete items[0].repository_key;
  assertThrows(
    () => parseRepositoryListResponse(stale),
    ".id is not part",
  );
});

Deno.test("repository API result converts stale payloads into bounded page errors", () => {
  const result = parseRepositoryListApiResult({
    data: {
      workspace_id: "w-a",
      items: { main: repositoryList.items[0] },
      source: "workspace-control-plane",
      diagnostics: [],
    },
    error: null,
  });
  if (result.data !== null) {
    throw new Error("stale payload must not reach the page");
  }
  if (!result.error?.includes("items must be an array")) {
    throw new Error(`unexpected bounded error: ${result.error}`);
  }
});

Deno.test("workspace response requires the permission projection", () => {
  const stale = {
    workspace_id: "w-a",
    display_name: "Alpha",
    record_authority: "workspace-control-plane",
    schema_version: 46,
    auth: {
      Passkey: {
        rp_id: "example.test",
        origin: "https://example.test",
        public_base_url: "https://example.test",
        cookie_name: "yoi_session",
      },
    },
    extension_points: {
      store: "sqlite",
      event_stream: { status: "available", note: "ready", diagnostics: [] },
      host_worker_bridge: {
        status: "available",
        note: "ready",
        diagnostics: [],
      },
      companion_console: {
        status: "available",
        note: "ready",
        diagnostics: [],
      },
    },
  };
  assertThrows(
    () => parseWorkspaceResponse(stale),
    "permissions must be an object",
  );
});

Deno.test("Repository settings consume the validated shared wire shape", async () => {
  const [loadSource, pageSource] = await Promise.all([
    Deno.readTextFile(
      new URL(
        "../src/routes/w/[workspaceId]/settings/repositories/+page.ts",
        import.meta.url,
      ),
    ),
    Deno.readTextFile(
      new URL(
        "../src/routes/w/[workspaceId]/settings/repositories/+page.svelte",
        import.meta.url,
      ),
    ),
  ]);

  for (
    const token of [
      "parseRepositoryListResponse",
      "repository.repository_key",
      "repository.observed_status",
      "sourceLabel(repository.source.kind)",
      "supportsRepositoryAccess(repository.source.kind)",
    ]
  ) {
    if (!loadSource.includes(token) && !pageSource.includes(token)) {
      throw new Error(`Repository settings should include ${token}`);
    }
  }
  for (
    const staleToken of [
      "repository.id",
      "repository.display_name",
      "repository.source.kind === 'remote_git'",
    ]
  ) {
    if (loadSource.includes(staleToken) || pageSource.includes(staleToken)) {
      throw new Error(`Repository settings must not use ${staleToken}`);
    }
  }
});
