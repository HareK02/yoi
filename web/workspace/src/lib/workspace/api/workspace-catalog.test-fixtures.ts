export const createdWorkspace = {
  workspace_id: "home-empty",
  owner_account_id: "fixture-owner",
  display_name: "New Workspace",
  state: "active",
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-02T00:00:00Z",
};

export const currentInitialRepository = {
  workspace_id: createdWorkspace.workspace_id,
  repository_key: "platform",
  kind: "git",
  provider: "git",
  source: { kind: "https", uri: "https://example.test/current.git" },
  default_ref: "main",
  source_fingerprint: "sha256:current",
  observed_status: "unverified",
  observed_at: null,
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-02T00:00:00Z",
};

export function creationResponse(repository: unknown = null, replayed = false) {
  return {
    workspace: createdWorkspace,
    repository,
    request_fingerprint: "sha256:creation",
    replayed,
  };
}
