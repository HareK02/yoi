// Synthetic public data only; shared by component tests and production browser fixtures.
export function metadataFixture(workspaceId = "home-owner") {
  return {
    workspace_id: workspaceId,
    display_name: workspaceId === "home-long"
      ? "Workspace with a long name — international documentation and distributed development"
      : "Workspace Settings Review",
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-02T00:00:00Z",
    source: "server_db",
    diagnostics: [],
  };
}
export function identityFixture(workspaceId = "home-owner", pending = false) {
  const identity = {
    workspace_id: workspaceId,
    key_id: "fixture-workspace-key",
    algorithm: "ed25519",
    state: pending ? "pending_provisioning" : "active",
    created_at: "2026-01-01T00:00:00Z",
  };
  if (pending) return { identity };
  const publicKey = "yoi-ed25519-pub:v1:synthetic-public-key";
  const fingerprint = "SHA256:synthetic-public-fingerprint-for-ui-review";
  return {
    identity: {
      ...identity,
      public_key: publicKey,
      public_key_fingerprint: fingerprint,
      provisioned_at: identity.created_at,
    },
    public_bundle: {
      workspace_id: workspaceId,
      backend_url: "https://workspace.example.test",
      key_id: identity.key_id,
      algorithm: identity.algorithm,
      public_key: publicKey,
      public_key_fingerprint: fingerprint,
    },
  };
}
export function deletionFixture(workspaceId = "home-owner") {
  return {
    workspace_id: workspaceId,
    display_name: metadataFixture(workspaceId).display_name,
    expected_workspace_updated_at: metadataFixture(workspaceId).updated_at,
    can_delete: true,
    blockers: [],
    resources: {
      workers: 2,
      workdirs: 1,
      repositories: 1,
      runtime_bindings: 1,
      secrets: 0,
      artifacts: 0,
    },
  };
}
export function settingsFixtureHandler() {
  const names = new Map<string, ReturnType<typeof metadataFixture>>();
  const provisioned = new Set<string>();
  return async (request: Request): Promise<Response | null> => {
    const url = new URL(request.url);
    const match = url.pathname.match(/^\/api\/w\/([^/]+)\/settings(.*)$/);
    if (match) {
      const [, id, path] = match;
      if (!path) {
        const current = names.get(id) ?? metadataFixture(id);
        if (request.method === "PUT") {
          const body = await request.json();
          if (body.expected_updated_at !== current.updated_at) {
            return new Response(
              "Workspace metadata changed. Reload the saved name.",
              { status: 409 },
            );
          }
          const next = {
            ...current,
            display_name: String(body.display_name).trim(),
            updated_at: new Date(Date.parse(current.updated_at) + 1).toISOString(),
          };
          names.set(id, next);
          return Response.json({ workspace: next, diagnostics: [] });
        }
        return Response.json(current);
      }
      if (path === "/signing-identity/provision") {
        provisioned.add(id);
        return Response.json(identityFixture(id));
      }
      if (path === "/signing-identity") {
        if (id === "home-error") {
          return new Response("Public identity is temporarily unavailable.", {
            status: 503,
          });
        }
        return Response.json(
          identityFixture(id, id === "home-empty" && !provisioned.has(id)),
        );
      }
    }
    const deletion = url.pathname.match(
      /^\/api\/workspaces\/([^/]+)\/deletion$/,
    );
    if (deletion) {
      if (request.method !== "GET") {
        return new Response("Fixture does not delete Workspaces.", {
          status: 409,
        });
      }
      return Response.json({
        ...deletionFixture(deletion[1]),
        ...(names.has(deletion[1])
          ? {
            display_name: names.get(deletion[1])!.display_name,
            expected_workspace_updated_at: names.get(deletion[1])!.updated_at,
          }
          : {}),
      });
    }
    return null;
  };
}
