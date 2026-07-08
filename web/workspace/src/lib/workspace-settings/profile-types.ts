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
