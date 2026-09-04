// Generated from workspace-api. Do not edit by hand.
// Regenerate: cargo run -q -p workspace-api --features typescript --example generate_skill_api_types > web/workspace/src/lib/generated/skill-api.ts

export const SKILL_API_AUTHORITY = "workspace-config-skills-v1" as const;

export const SKILL_API_LIMITS = {
  maxSafeInteger: 9007199254740991,
  maxCatalogEntries: 500,
  maxOverrides: 64,
  maxDiagnostics: 100,
  maxResources: 500,
  maxAllowedTools: 100,
  maxNameBytes: 128,
  maxLabelBytes: 4096,
  maxBodyBytes: 1048576,
  maxPathBytes: 1024,
  maxDigestBytes: 128,
  maxResponseBytes: 2097152,
} as const;

export type SkillDiagnosticSeverity = "error" | "warning";

export type SkillDiagnostic = {
  severity: SkillDiagnosticSeverity;
  code: string;
  message: string;
  source?: string;
};

export type SkillSourceKind = "builtin" | "workspace";

export type SkillProvenance = {
  kind: SkillSourceKind;
  id: string;
  virtual_path?: string;
  revision?: number;
  source_digest?: string;
  tree_digest?: string;
};

export type SkillActivationStatus = "active" | "inactive";

export type SkillProjectionStatus = "valid" | "invalid";

export type SkillProjectionIdentity = {
  config_revision: number;
  tree_digest: string;
};

export type SkillResourceRef = {
  kind: string;
  name: string;
  supported: boolean;
  diagnostic?: string;
};

export type SkillCatalogEntry = {
  name: string;
  description: string;
  activation_status: SkillActivationStatus;
  projection_status: SkillProjectionStatus;
  provenance: SkillProvenance;
  overrides: Array<SkillProvenance>;
  diagnostics: Array<SkillDiagnostic>;
};

export type SkillCatalogResponse = {
  authority: string;
  projection: SkillProjectionIdentity;
  entries: Array<SkillCatalogEntry>;
  diagnostics: Array<SkillDiagnostic>;
};

export type SkillDetailResponse = {
  authority: string;
  projection: SkillProjectionIdentity;
  name: string;
  description: string;
  provenance: SkillProvenance;
  overrides: Array<SkillProvenance>;
  diagnostics: Array<SkillDiagnostic>;
  activation_status: SkillActivationStatus;
  projection_status: SkillProjectionStatus;
  body: string;
  allowed_tools: Array<string>;
  allowed_tools_status: string;
  resources: Array<SkillResourceRef>;
};
