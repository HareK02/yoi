export type VirtualPath = string;

export type ConfigContentType = "decodal" | "text";

export interface ConfigEntry {
  path: VirtualPath;
  content_type: ConfigContentType;
  content: string;
  content_digest: string;
}

export interface ConfigTreeSnapshot {
  revision: number;
  digest: string;
  entries: Record<VirtualPath, ConfigEntry>;
}

export type ConfigTreeChange =
  | {
    kind: "create";
    path: VirtualPath;
    content_type: ConfigContentType;
    content: string;
  }
  | {
    kind: "update";
    path: VirtualPath;
    expected_digest: string;
    content: string;
  }
  | {
    kind: "rename";
    from: VirtualPath;
    to: VirtualPath;
    expected_digest: string;
  }
  | {
    kind: "delete";
    path: VirtualPath;
    expected_digest: string;
  };

export interface ToolchainContract {
  contract_version: number;
  decodal_version: string;
  schema_version: number;
  entrypoints: VirtualPath[];
  import_policy_version: number;
  fingerprint: string;
}

export interface ConfigDiagnostic {
  path: VirtualPath;
  revision: number;
  tree_digest: string;
  kind: string;
  span: { start_byte: number; end_byte: number };
  message: string;
  labels: Array<{
    span: { start_byte: number; end_byte: number };
    message: string;
  }>;
  notes: string[];
}

export interface WorkspaceConfigTreeResponse {
  snapshot: ConfigTreeSnapshot;
  contract: ToolchainContract;
  projection_digest: string;
}

export interface EvaluatedConfigCandidate {
  base_revision: number;
  base_digest: string;
  snapshot: ConfigTreeSnapshot;
  contract: ToolchainContract;
  evaluation: {
    projections: Array<{
      entrypoint: VirtualPath;
      data_json: unknown;
      projection_digest: string;
    }>;
    projection_digest: string;
  };
}

export interface ConfigCommitRequest {
  base_revision: number;
  base_digest: string;
  changes: ConfigTreeChange[];
  entrypoints: VirtualPath[];
}
