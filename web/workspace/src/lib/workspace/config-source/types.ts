// Workspace REST DTOs are generated from the server-api Rust authority.
export type {
  ConfigCommitRequest,
  ConfigContentType,
  ConfigEntry,
  ConfigProjectionValidator,
  ConfigSchemaContribution,
  ConfigTreeChange,
  ConfigTreeSnapshot,
  ToolchainContract,
  WorkspaceConfigSchemaBundle,
  WorkspaceConfigTreeResponse,
} from "$lib/generated/legacy-server-api.ts";

// Browser-local Decodal/WASM projections remain owned by config-source.
export type { ConfigDiagnostic } from "./generated/types/ConfigDiagnostic.ts";
export type { ConfigDiagnosticLabel } from "./generated/types/ConfigDiagnosticLabel.ts";
export type { ConfigSpan } from "./generated/types/ConfigSpan.ts";
export type { EvaluatedProjection } from "./generated/types/EvaluatedProjection.ts";
export type { EvaluationResult } from "./generated/types/EvaluationResult.ts";
export type { VirtualPath } from "./generated/types/VirtualPath.ts";
