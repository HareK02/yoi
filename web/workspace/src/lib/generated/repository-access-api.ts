// Generated from workspace-api. Do not edit by hand.
// Regenerate: cargo run -q -p workspace-api --features typescript --example generate_repository_access_types > web/workspace/src/lib/generated/repository-access-api.ts

export type RepositorySshCredential = {
  credential_id: string;
  workspace_id: string;
  name: string;
  public_key_algorithm: string;
  public_key_fingerprint: string;
  current_revision: number;
  status: string;
  created_at: string;
  rotated_at: string | null;
  referenced_repositories: Array<string>;
};

export type CreateRepositorySshCredentialRequest = {
  operation_id: string;
  credential_id: string;
  name: string;
  private_key: string;
  passphrase: string | null;
};

export type RotateRepositorySshCredentialRequest = {
  operation_id: string;
  expected_revision: number;
  private_key: string;
  passphrase: string | null;
};

export type DeleteRepositorySshCredentialRequest = {
  operation_id: string;
  expected_revision: number;
};

export type RepositorySshHostTrust = {
  host_trust_id: string;
  workspace_id: string;
  hostname: string;
  port: number;
  key_algorithm: string;
  host_key: string;
  fingerprint: string;
  current_revision: number;
  created_at: string;
  updated_at: string;
  referenced_repositories: Array<string>;
};

export type PutRepositorySshHostTrustRequest = {
  operation_id: string;
  host_trust_id: string;
  hostname: string;
  port: number;
  host_key: string;
  expected_revision: number | null;
};

export type DeleteRepositorySshHostTrustRequest = {
  operation_id: string;
  expected_revision: number;
};

export type RepositoryAccessMode = "read_only" | "read_write";

export type RepositorySshAccessBinding = {
  repository_id: string;
  credential_id: string;
  host_trust_id: string;
  access: RepositoryAccessMode;
};

export type RepositoryAccessProjection = {
  workspace_id: string;
  config_revision: number;
  projection_digest: string;
  bindings: Array<RepositorySshAccessBinding>;
};
