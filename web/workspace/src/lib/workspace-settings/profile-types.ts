import type { Diagnostic } from "./model";

export type WorkspaceMetadataSettingsResponse = {
  workspace_id: string;
  display_name: string;
  created_at: string;
  revision: string;
  source: string;
  diagnostics: Diagnostic[];
};

export type WorkspaceMetadataMutationResponse = {
  workspace: WorkspaceMetadataSettingsResponse;
  diagnostics: Diagnostic[];
};

export type WorkspaceProfileSummary = {
  profile_id: string;
  selector: string;
  label: string;
  source_kind: "builtin" | "project" | string;
  profile_source_id?: string | null;
  description?: string | null;
  editable: boolean;
  is_default: boolean;
  diagnostics: Diagnostic[];
};

export type WorkspaceProfileSourceSummary = {
  profile_source_id: string;
  display_path: string;
  kind: "decodal" | string;
  content_type: string;
  content_digest: string;
  provenance: "project_profile_source_tree" | string;
  editable: boolean;
  revision: string;
  size_bytes: number;
  diagnostics: Diagnostic[];
};

export type ProfileSettingsResponse = {
  workspace_id: string;
  registry_revision: string;
  default_profile?: string | null;
  profiles: WorkspaceProfileSummary[];
  sources: WorkspaceProfileSourceSummary[];
  source_trees: WorkspaceProfileSourceTreeSummary[];
  diagnostics: Diagnostic[];
};

export type WorkspaceProfileSourceDetailResponse = {
  workspace_id: string;
  profile: WorkspaceProfileSummary;
  source: WorkspaceProfileSourceSummary;
  content: string;
  diagnostics: Diagnostic[];
};

export type ProfileSettingsMutationResponse = {
  workspace_id: string;
  settings: ProfileSettingsResponse;
  diagnostics: Diagnostic[];
};

export type WorkspaceProfileSourceTreeSummary = {
  source_tree_id: string;
  label: string;
  root_path: string;
  kind: "decodal_source_tree" | string;
  content_type: string;
  content_digest: string;
  provenance: "project_profile_source_tree" | string;
  editable: boolean;
  revision: string;
  file_count: number;
  diagnostics: Diagnostic[];
};

export type WorkspaceProfileSourceTreeFileSummary = {
  path: string;
  kind: "decodal" | string;
  content_type: string;
  content_digest: string;
  provenance: "project_profile_source_tree" | string;
  editable: boolean;
  revision: string;
  size_bytes: number;
  diagnostics: Diagnostic[];
};

export type WorkspaceProfileSourceTreeResponse = {
  workspace_id: string;
  tree: WorkspaceProfileSourceTreeSummary;
  files: WorkspaceProfileSourceTreeFileSummary[];
  diagnostics: Diagnostic[];
};

export type WorkspaceProfileSourceTreeFileResponse = {
  workspace_id: string;
  source_tree_id: string;
  file: WorkspaceProfileSourceTreeFileSummary;
  content: string;
  diagnostics: Diagnostic[];
};
