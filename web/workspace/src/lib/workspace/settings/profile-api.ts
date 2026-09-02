import type {
  Diagnostic,
  DiagnosticSeverity,
  ProfileSettingsResponse,
  UpdateWorkspaceMetadataRequest,
  WorkspaceMetadataMutationResponse,
  WorkspaceMetadataSettingsResponse,
  WorkspaceProfileSourceProvenance,
  WorkspaceProfileSourceSummary,
  WorkspaceProfileSummary,
} from "$lib/generated/workspace-api";

export class ProfileApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
    this.name = "ProfileApiError";
  }
}

type JsonRecord = Record<string, unknown>;

function record(value: unknown, context: string): JsonRecord {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new ProfileApiError(`${context} returned an invalid response.`, 502);
  }
  return value as JsonRecord;
}

function exactKeys(
  value: JsonRecord,
  required: readonly string[],
  optional: readonly string[],
  context: string,
): void {
  const allowed = new Set([...required, ...optional]);
  if (
    required.some((key) => !(key in value)) ||
    Object.keys(value).some((key) => !allowed.has(key))
  ) {
    throw new ProfileApiError(`${context} returned an invalid response.`, 502);
  }
}

function stringValue(value: unknown, context: string): string {
  if (typeof value !== "string") {
    throw new ProfileApiError(`${context} returned an invalid response.`, 502);
  }
  return value;
}

function booleanValue(value: unknown, context: string): boolean {
  if (typeof value !== "boolean") {
    throw new ProfileApiError(`${context} returned an invalid response.`, 502);
  }
  return value;
}

function optionalString(
  value: unknown,
  context: string,
): string | null | undefined {
  if (value === undefined || value === null) return value;
  return stringValue(value, context);
}

function optionalRevision(
  value: unknown,
  context: string,
): number | null | undefined {
  if (value === undefined || value === null) return value;
  if (!Number.isSafeInteger(value) || (value as number) < 0) {
    throw new ProfileApiError(`${context} returned an invalid response.`, 502);
  }
  return value as number;
}

function arrayValue<T>(
  value: unknown,
  parser: (item: unknown) => T,
  context: string,
): T[] {
  if (!Array.isArray(value)) {
    throw new ProfileApiError(`${context} returned an invalid response.`, 502);
  }
  return value.map(parser);
}

function parseDiagnostic(value: unknown): Diagnostic {
  const item = record(value, "Workspace settings");
  exactKeys(item, ["code", "severity", "message"], [], "Workspace settings");
  const severity = stringValue(item.severity, "Workspace settings");
  if (!(["info", "warning", "error"] as string[]).includes(severity)) {
    throw new ProfileApiError(
      "Workspace settings returned an invalid response.",
      502,
    );
  }
  return {
    code: stringValue(item.code, "Workspace settings"),
    severity: severity as DiagnosticSeverity,
    message: stringValue(item.message, "Workspace settings"),
  };
}

export function parseWorkspaceMetadataSettingsResponse(
  value: unknown,
): WorkspaceMetadataSettingsResponse {
  const item = record(value, "Workspace metadata");
  exactKeys(
    item,
    [
      "workspace_id",
      "display_name",
      "created_at",
      "revision",
      "source",
      "diagnostics",
    ],
    [],
    "Workspace metadata",
  );
  return {
    workspace_id: stringValue(item.workspace_id, "Workspace metadata"),
    display_name: stringValue(item.display_name, "Workspace metadata"),
    created_at: stringValue(item.created_at, "Workspace metadata"),
    revision: stringValue(item.revision, "Workspace metadata"),
    source: stringValue(item.source, "Workspace metadata"),
    diagnostics: arrayValue(
      item.diagnostics,
      parseDiagnostic,
      "Workspace metadata",
    ),
  };
}

export function parseWorkspaceMetadataMutationResponse(
  value: unknown,
): WorkspaceMetadataMutationResponse {
  const item = record(value, "Workspace metadata update");
  exactKeys(
    item,
    ["workspace", "diagnostics"],
    [],
    "Workspace metadata update",
  );
  return {
    workspace: parseWorkspaceMetadataSettingsResponse(item.workspace),
    diagnostics: arrayValue(
      item.diagnostics,
      parseDiagnostic,
      "Workspace metadata update",
    ),
  };
}

function parseWorkspaceProfileSummary(value: unknown): WorkspaceProfileSummary {
  const item = record(value, "Profile catalog");
  exactKeys(
    item,
    [
      "profile_id",
      "selector",
      "label",
      "source_kind",
      "editable",
      "is_default",
      "diagnostics",
    ],
    ["profile_source_id", "description"],
    "Profile catalog",
  );
  return {
    profile_id: stringValue(item.profile_id, "Profile catalog"),
    selector: stringValue(item.selector, "Profile catalog"),
    label: stringValue(item.label, "Profile catalog"),
    source_kind: stringValue(item.source_kind, "Profile catalog"),
    profile_source_id: optionalString(
      item.profile_source_id,
      "Profile catalog",
    ),
    description: optionalString(item.description, "Profile catalog"),
    editable: booleanValue(item.editable, "Profile catalog"),
    is_default: booleanValue(item.is_default, "Profile catalog"),
    diagnostics: arrayValue(
      item.diagnostics,
      parseDiagnostic,
      "Profile catalog",
    ),
  };
}

function parseWorkspaceProfileSourceSummary(
  value: unknown,
): WorkspaceProfileSourceSummary {
  const item = record(value, "Profile source catalog");
  exactKeys(
    item,
    [
      "profile_source_id",
      "display_path",
      "kind",
      "content_type",
      "content_digest",
      "provenance",
      "editable",
      "revision",
      "size_bytes",
      "diagnostics",
    ],
    [],
    "Profile source catalog",
  );
  const provenance = stringValue(item.provenance, "Profile source catalog");
  if (provenance !== "project_profile_source_tree") {
    throw new ProfileApiError(
      "Profile source catalog returned an invalid response.",
      502,
    );
  }
  const sizeBytes = optionalRevision(item.size_bytes, "Profile source catalog");
  if (sizeBytes === undefined || sizeBytes === null) {
    throw new ProfileApiError(
      "Profile source catalog returned an invalid response.",
      502,
    );
  }
  return {
    profile_source_id: stringValue(
      item.profile_source_id,
      "Profile source catalog",
    ),
    display_path: stringValue(item.display_path, "Profile source catalog"),
    kind: stringValue(item.kind, "Profile source catalog"),
    content_type: stringValue(item.content_type, "Profile source catalog"),
    content_digest: stringValue(item.content_digest, "Profile source catalog"),
    provenance: provenance as WorkspaceProfileSourceProvenance,
    editable: booleanValue(item.editable, "Profile source catalog"),
    revision: stringValue(item.revision, "Profile source catalog"),
    size_bytes: sizeBytes,
    diagnostics: arrayValue(
      item.diagnostics,
      parseDiagnostic,
      "Profile source catalog",
    ),
  };
}

export function parseProfileSettingsResponse(
  value: unknown,
): ProfileSettingsResponse {
  const item = record(value, "Profile settings");
  exactKeys(
    item,
    ["workspace_id", "registry_revision", "profiles", "sources", "diagnostics"],
    ["config_revision", "tree_digest", "projection_digest", "default_profile"],
    "Profile settings",
  );
  return {
    workspace_id: stringValue(item.workspace_id, "Profile settings"),
    registry_revision: stringValue(item.registry_revision, "Profile settings"),
    config_revision: optionalRevision(item.config_revision, "Profile settings"),
    tree_digest: optionalString(item.tree_digest, "Profile settings"),
    projection_digest: optionalString(
      item.projection_digest,
      "Profile settings",
    ),
    default_profile: optionalString(item.default_profile, "Profile settings"),
    profiles: arrayValue(
      item.profiles,
      parseWorkspaceProfileSummary,
      "Profile settings",
    ),
    sources: arrayValue(
      item.sources,
      parseWorkspaceProfileSourceSummary,
      "Profile settings",
    ),
    diagnostics: arrayValue(
      item.diagnostics,
      parseDiagnostic,
      "Profile settings",
    ),
  };
}

async function parseResponse<T>(
  response: Response,
  parser: (value: unknown) => T,
): Promise<T> {
  if (!response.ok) {
    throw new ProfileApiError(
      (await response.text()) || response.statusText,
      response.status,
    );
  }
  return parser(await response.json() as unknown);
}

export async function fetchWorkspaceMetadata(
  workspaceId: string,
): Promise<WorkspaceMetadataSettingsResponse> {
  return await parseResponse(
    await fetch(`/api/w/${encodeURIComponent(workspaceId)}/settings/workspace`),
    parseWorkspaceMetadataSettingsResponse,
  );
}

export async function updateWorkspaceMetadata(
  workspaceId: string,
  request: UpdateWorkspaceMetadataRequest,
): Promise<WorkspaceMetadataMutationResponse> {
  return await parseResponse(
    await fetch(
      `/api/w/${encodeURIComponent(workspaceId)}/settings/workspace`,
      {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(request),
      },
    ),
    parseWorkspaceMetadataMutationResponse,
  );
}

export async function fetchProfileSettings(
  workspaceId: string,
): Promise<ProfileSettingsResponse> {
  return await parseResponse(
    await fetch(`/api/w/${encodeURIComponent(workspaceId)}/settings/profiles`),
    parseProfileSettingsResponse,
  );
}
