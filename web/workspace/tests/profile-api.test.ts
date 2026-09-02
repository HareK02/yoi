declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

function assertEquals(actual: unknown, expected: unknown): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

function assertThrows<T extends Error>(
  fn: () => unknown,
  errorType: abstract new (...args: never[]) => T,
): void {
  try {
    fn();
  } catch (error) {
    if (error instanceof errorType) return;
    throw error;
  }
  throw new Error(`expected ${errorType.name} to be thrown`);
}

import {
  fetchProfileSettings,
  fetchWorkspaceMetadata,
  parseProfileSettingsResponse,
  parseWorkspaceMetadataSettingsResponse,
  ProfileApiError,
  updateWorkspaceMetadata,
} from "../src/lib/workspace/settings/profile-api.ts";

const diagnostic = {
  code: "ok",
  severity: "info",
  message: "ready",
};

function profileSettingsFixture(): Record<string, unknown> {
  return {
    workspace_id: "workspace 1",
    registry_revision: "config-source:7:tree:projection",
    config_revision: 7,
    tree_digest: "tree",
    projection_digest: "projection",
    default_profile: "workspace:coder",
    profiles: [{
      profile_id: "workspace:coder",
      selector: "workspace:coder",
      label: "Coder",
      source_kind: "project",
      profile_source_id: "profile-source-1",
      description: null,
      editable: true,
      is_default: true,
      diagnostics: [],
    }],
    sources: [{
      profile_source_id: "profile-source-1",
      display_path: "profiles/coder.dcdl",
      kind: "profile",
      content_type: "text/x-decodal",
      content_digest: "sha256:source",
      provenance: "project_profile_source_tree",
      editable: false,
      revision: "config-source:7",
      size_bytes: 128,
      diagnostics: [],
    }],
    diagnostics: [diagnostic],
  };
}

Deno.test("profile settings requests use scoped API and strictly validate responses", async () => {
  const originalFetch = globalThis.fetch;
  const requests: Array<{ url: string; init?: RequestInit }> = [];
  globalThis.fetch = (input: string | URL | Request, init?: RequestInit) => {
    requests.push({ url: String(input), init });
    return Promise.resolve(Response.json(profileSettingsFixture()));
  };

  try {
    const response = await fetchProfileSettings("workspace 1");
    assertEquals(response.config_revision, 7);
    assertEquals(response.sources[0].provenance, "project_profile_source_tree");
    assertEquals(requests.length, 1);
    assertEquals(requests[0].url, "/api/w/workspace%201/settings/profiles");
    assertEquals(requests[0].init, undefined);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

Deno.test("workspace metadata requests use generated DTO shapes", async () => {
  const originalFetch = globalThis.fetch;
  const requests: Array<{ url: string; init?: RequestInit }> = [];
  const workspace = {
    workspace_id: "workspace 1",
    display_name: "Workspace",
    created_at: "2026-01-01T00:00:00Z",
    revision: "sha256:metadata",
    source: "workspace-config",
    diagnostics: [diagnostic],
  };
  globalThis.fetch = (input: string | URL | Request, init?: RequestInit) => {
    requests.push({ url: String(input), init });
    return Promise.resolve(
      Response.json(
        init?.method === "PUT" ? { workspace, diagnostics: [] } : workspace,
      ),
    );
  };

  try {
    assertEquals(
      (await fetchWorkspaceMetadata("workspace 1")).revision,
      "sha256:metadata",
    );
    assertEquals(
      (await updateWorkspaceMetadata("workspace 1", {
        display_name: "Renamed",
        revision: "sha256:metadata",
      })).workspace.workspace_id,
      "workspace 1",
    );
    assertEquals(requests.map((request) => request.url), [
      "/api/w/workspace%201/settings/workspace",
      "/api/w/workspace%201/settings/workspace",
    ]);
    assertEquals(requests[1].init?.method, "PUT");
    assertEquals(
      requests[1].init?.body,
      JSON.stringify({ display_name: "Renamed", revision: "sha256:metadata" }),
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});

Deno.test("profile settings parser rejects missing, mistyped, stale, and invalid provenance fields", () => {
  const missing = profileSettingsFixture();
  delete missing.profiles;
  assertThrows(
    () => parseProfileSettingsResponse(missing),
    ProfileApiError,
  );

  const mistyped = profileSettingsFixture();
  mistyped.config_revision = "7";
  assertThrows(
    () => parseProfileSettingsResponse(mistyped),
    ProfileApiError,
  );

  const stale = profileSettingsFixture();
  stale.legacy_profile_directory = ".yoi/profiles";
  assertThrows(
    () => parseProfileSettingsResponse(stale),
    ProfileApiError,
  );

  const invalidProvenance = profileSettingsFixture();
  const sources = invalidProvenance.sources as Array<Record<string, unknown>>;
  sources[0].provenance = "filesystem";
  assertThrows(
    () => parseProfileSettingsResponse(invalidProvenance),
    ProfileApiError,
  );
});

Deno.test("workspace metadata parser rejects incomplete or stale response fields", () => {
  assertThrows(
    () =>
      parseWorkspaceMetadataSettingsResponse({
        workspace_id: "workspace-test",
        display_name: "Workspace",
      }),
    ProfileApiError,
  );
  assertThrows(
    () =>
      parseWorkspaceMetadataSettingsResponse({
        workspace_id: "workspace-test",
        display_name: "Workspace",
        created_at: "2026-01-01T00:00:00Z",
        revision: "sha256:metadata",
        source: "workspace-config",
        diagnostics: [],
        workspace_path: "/legacy/path",
      }),
    ProfileApiError,
  );
});
