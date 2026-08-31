import {
  parseRepositoryAccessProjection,
  parseRepositorySshCredentials,
  parseRepositorySshHostTrusts,
  RepositoryAccessSchemaError,
} from "../../src/lib/workspace/api/repository-access.ts";

function assertEquals(actual: unknown, expected: unknown): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

function assertSchemaError(body: () => unknown, path: string): void {
  try {
    body();
  } catch (error) {
    if (!(error instanceof RepositoryAccessSchemaError)) {
      throw error;
    }
    if (!error.message.includes(path)) {
      throw new Error(
        `expected schema error path ${path}, got ${error.message}`,
      );
    }
    return;
  }
  throw new Error(`expected RepositoryAccessSchemaError for ${path}`);
}

const credential = {
  credential_id: "deploy-key",
  workspace_id: "workspace-1",
  name: "Deploy key",
  public_key_algorithm: "ssh-ed25519",
  public_key_fingerprint: "SHA256:credential",
  current_revision: 2,
  status: "active",
  created_at: "2026-09-01T00:00:00Z",
  rotated_at: null,
  referenced_repositories: ["main"],
};

const hostTrust = {
  host_trust_id: "gitea",
  workspace_id: "workspace-1",
  hostname: "gitea.example.test",
  port: 22,
  key_algorithm: "ssh-ed25519",
  host_key: "ssh-ed25519 AAAA",
  fingerprint: "SHA256:host",
  current_revision: 3,
  created_at: "2026-09-01T00:00:00Z",
  updated_at: "2026-09-02T00:00:00Z",
  referenced_repositories: ["main"],
};

Deno.test("Repository Access parsers accept generated response contracts", () => {
  assertEquals(parseRepositorySshCredentials([credential]), [credential]);
  assertEquals(parseRepositorySshHostTrusts([hostTrust]), [hostTrust]);
  assertEquals(
    parseRepositoryAccessProjection({
      workspace_id: "workspace-1",
      config_revision: 4,
      projection_digest: "sha256:projection",
      bindings: [{
        repository_id: "main",
        credential_id: "deploy-key",
        host_trust_id: "gitea",
        access: "read_only",
      }],
    }),
    {
      workspace_id: "workspace-1",
      config_revision: 4,
      projection_digest: "sha256:projection",
      bindings: [{
        repository_id: "main",
        credential_id: "deploy-key",
        host_trust_id: "gitea",
        access: "read_only",
      }],
    },
  );
});

Deno.test("Repository Access parsers reject malformed list responses", () => {
  assertSchemaError(
    () => parseRepositorySshCredentials({ credentials: [credential] }),
    "credentials",
  );
  assertSchemaError(
    () => parseRepositorySshHostTrusts({ host_trusts: [hostTrust] }),
    "host_trusts",
  );
});

Deno.test("Repository Access parsers reject missing and wrong-typed fields", () => {
  const { current_revision: _revision, ...missingRevision } = credential;
  assertSchemaError(
    () => parseRepositorySshCredentials([missingRevision]),
    "credentials[0].current_revision",
  );
  assertSchemaError(
    () => parseRepositorySshHostTrusts([{ ...hostTrust, port: "22" }]),
    "host_trusts[0].port",
  );
  assertSchemaError(
    () =>
      parseRepositoryAccessProjection({
        workspace_id: "workspace-1",
        config_revision: 4,
        projection_digest: "sha256:projection",
        bindings: [{
          repository_id: "main",
          credential_id: "deploy-key",
          host_trust_id: "gitea",
          access: "admin",
        }],
      }),
    "access_projection.bindings[0].access",
  );
});

Deno.test("Repository Access parsers reject unknown response fields", () => {
  assertSchemaError(
    () =>
      parseRepositorySshCredentials([{ ...credential, private_key: "secret" }]),
    "credentials[0].private_key",
  );
});
