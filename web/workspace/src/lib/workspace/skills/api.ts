import {
  SKILL_API_AUTHORITY,
  SKILL_API_LIMITS,
  type SkillActivationStatus,
  type SkillCatalogEntry,
  type SkillCatalogResponse,
  type SkillDetailResponse,
  type SkillDiagnostic,
  type SkillDiagnosticSeverity,
  type SkillProjectionIdentity,
  type SkillProjectionStatus,
  type SkillProvenance,
  type SkillResourceRef,
  type SkillSourceKind,
} from "$lib/generated/skill-api.ts";

export class SkillApiContractError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "SkillApiContractError";
  }
}

export function parseSkillCatalogResponse(
  value: unknown,
): SkillCatalogResponse {
  const record = strictObject(value, [
    "authority",
    "projection",
    "entries",
    "diagnostics",
  ], "Skill catalog response");
  const authority = boundedString(
    record.authority,
    "Skill catalog authority",
    SKILL_API_LIMITS.maxLabelBytes,
    false,
  );
  if (authority !== SKILL_API_AUTHORITY) {
    throw contractError("unsupported Skill catalog authority");
  }
  const projection = parseProjection(record.projection);
  return {
    authority,
    projection,
    entries: boundedArray(
      record.entries,
      "Skill catalog entries",
      SKILL_API_LIMITS.maxCatalogEntries,
    ).map((entry) => parseCatalogEntry(entry, projection)),
    diagnostics: parseDiagnostics(record.diagnostics),
  };
}

export function parseSkillDetailResponse(value: unknown): SkillDetailResponse {
  const record = strictObject(value, [
    "authority",
    "projection",
    "name",
    "description",
    "provenance",
    "overrides",
    "diagnostics",
    "activation_status",
    "projection_status",
    "body",
    "allowed_tools",
    "allowed_tools_status",
    "resources",
  ], "Skill detail response");
  const authority = boundedString(
    record.authority,
    "Skill detail authority",
    SKILL_API_LIMITS.maxLabelBytes,
    false,
  );
  if (authority !== SKILL_API_AUTHORITY) {
    throw contractError("unsupported Skill detail authority");
  }
  const projection = parseProjection(record.projection);
  return {
    authority,
    projection,
    name: boundedString(
      record.name,
      "Skill name",
      SKILL_API_LIMITS.maxNameBytes,
      false,
    ),
    description: boundedString(
      record.description,
      "Skill description",
      SKILL_API_LIMITS.maxLabelBytes,
      true,
    ),
    provenance: parseProvenance(record.provenance, projection),
    overrides: parseProvenances(record.overrides, projection),
    diagnostics: parseDiagnostics(record.diagnostics),
    activation_status: activationStatus(record.activation_status),
    projection_status: projectionStatus(record.projection_status),
    body: boundedString(
      record.body,
      "Skill body",
      SKILL_API_LIMITS.maxBodyBytes,
      true,
    ),
    allowed_tools: boundedArray(
      record.allowed_tools,
      "Skill allowed tools",
      SKILL_API_LIMITS.maxAllowedTools,
    ).map((tool) =>
      boundedString(
        tool,
        "Skill allowed tool",
        SKILL_API_LIMITS.maxLabelBytes,
        false,
      )
    ),
    allowed_tools_status: boundedString(
      record.allowed_tools_status,
      "Skill allowed-tools status",
      SKILL_API_LIMITS.maxLabelBytes,
      false,
    ),
    resources: boundedArray(
      record.resources,
      "Skill resources",
      SKILL_API_LIMITS.maxResources,
    ).map(parseResource),
  };
}

function parseCatalogEntry(
  value: unknown,
  projection: SkillProjectionIdentity,
): SkillCatalogEntry {
  const record = strictObject(value, [
    "name",
    "description",
    "activation_status",
    "projection_status",
    "provenance",
    "overrides",
    "diagnostics",
  ], "Skill catalog entry");
  return {
    name: boundedString(
      record.name,
      "Skill name",
      SKILL_API_LIMITS.maxNameBytes,
      false,
    ),
    description: boundedString(
      record.description,
      "Skill description",
      SKILL_API_LIMITS.maxLabelBytes,
      true,
    ),
    activation_status: activationStatus(record.activation_status),
    projection_status: projectionStatus(record.projection_status),
    provenance: parseProvenance(record.provenance, projection),
    overrides: parseProvenances(record.overrides, projection),
    diagnostics: parseDiagnostics(record.diagnostics),
  };
}

function parseProjection(value: unknown): SkillProjectionIdentity {
  const record = strictObject(
    value,
    ["config_revision", "tree_digest"],
    "Skill projection identity",
  );
  return {
    config_revision: safeInteger(
      record.config_revision,
      "Skill config revision",
    ),
    tree_digest: boundedString(
      record.tree_digest,
      "Skill tree digest",
      SKILL_API_LIMITS.maxDigestBytes,
      false,
    ),
  };
}

function parseProvenances(
  value: unknown,
  projection: SkillProjectionIdentity,
): SkillProvenance[] {
  return boundedArray(
    value,
    "Skill overrides",
    SKILL_API_LIMITS.maxOverrides,
  ).map((provenance) => parseProvenance(provenance, projection));
}

function parseProvenance(
  value: unknown,
  projection: SkillProjectionIdentity,
): SkillProvenance {
  const record = strictObject(
    value,
    [
      "kind",
      "id",
      "virtual_path",
      "revision",
      "source_digest",
      "tree_digest",
    ],
    "Skill provenance",
    [
      "virtual_path",
      "revision",
      "source_digest",
      "tree_digest",
    ],
  );
  const kind = sourceKind(record.kind);
  const id = boundedString(
    record.id,
    "Skill provenance id",
    SKILL_API_LIMITS.maxLabelBytes,
    false,
  );
  const virtualPath = optionalBoundedString(
    record.virtual_path,
    "Skill virtual path",
    SKILL_API_LIMITS.maxPathBytes,
  );
  const sourceDigest = optionalBoundedString(
    record.source_digest,
    "Skill source digest",
    SKILL_API_LIMITS.maxDigestBytes,
  );
  const treeDigest = optionalBoundedString(
    record.tree_digest,
    "Skill provenance tree digest",
    SKILL_API_LIMITS.maxDigestBytes,
  );
  const revision = record.revision === undefined
    ? undefined
    : safeInteger(record.revision, "Skill provenance revision");

  if (
    !id.startsWith(`${kind}:`) || virtualPath === undefined ||
    sourceDigest === undefined || !isVirtualPath(virtualPath)
  ) {
    throw contractError("invalid Skill provenance");
  }
  if (kind === "builtin") {
    if (revision !== undefined || treeDigest !== undefined) {
      throw contractError("invalid built-in Skill provenance");
    }
  } else {
    if (revision === undefined || treeDigest === undefined) {
      throw contractError("incomplete Workspace Skill provenance");
    }
    if (
      revision !== projection.config_revision ||
      treeDigest !== projection.tree_digest
    ) {
      throw contractError("stale Workspace Skill projection");
    }
  }

  return {
    kind,
    id,
    virtual_path: virtualPath,
    revision,
    source_digest: sourceDigest,
    tree_digest: treeDigest,
  };
}

function parseDiagnostics(value: unknown): SkillDiagnostic[] {
  return boundedArray(
    value,
    "Skill diagnostics",
    SKILL_API_LIMITS.maxDiagnostics,
  ).map((diagnostic) => {
    const record = strictObject(
      diagnostic,
      [
        "severity",
        "code",
        "message",
        "source",
      ],
      "Skill diagnostic",
      ["source"],
    );
    return {
      severity: diagnosticSeverity(record.severity),
      code: boundedString(
        record.code,
        "Skill diagnostic code",
        SKILL_API_LIMITS.maxLabelBytes,
        false,
      ),
      message: boundedString(
        record.message,
        "Skill diagnostic message",
        SKILL_API_LIMITS.maxLabelBytes,
        false,
      ),
      source: optionalBoundedString(
        record.source,
        "Skill diagnostic source",
        SKILL_API_LIMITS.maxPathBytes,
      ),
    };
  });
}

function parseResource(value: unknown): SkillResourceRef {
  const record = strictObject(
    value,
    [
      "kind",
      "name",
      "supported",
      "diagnostic",
    ],
    "Skill resource",
    ["diagnostic"],
  );
  if (typeof record.supported !== "boolean") {
    throw contractError("Skill resource supported must be a boolean");
  }
  const name = boundedString(
    record.name,
    "Skill resource name",
    SKILL_API_LIMITS.maxPathBytes,
    false,
  );
  if (!isVirtualPath(name)) {
    throw contractError("invalid Skill resource virtual path");
  }
  return {
    kind: boundedString(
      record.kind,
      "Skill resource kind",
      SKILL_API_LIMITS.maxLabelBytes,
      false,
    ),
    name,
    supported: record.supported,
    diagnostic: optionalBoundedString(
      record.diagnostic,
      "Skill resource diagnostic",
      SKILL_API_LIMITS.maxLabelBytes,
    ),
  };
}

function sourceKind(value: unknown): SkillSourceKind {
  if (value === "builtin" || value === "workspace") return value;
  throw contractError("unsupported Skill provenance kind");
}

function diagnosticSeverity(value: unknown): SkillDiagnosticSeverity {
  if (value === "error" || value === "warning") return value;
  throw contractError("unsupported Skill diagnostic severity");
}

function activationStatus(value: unknown): SkillActivationStatus {
  if (value === "active" || value === "inactive") return value;
  throw contractError("unsupported Skill activation status");
}

function projectionStatus(value: unknown): SkillProjectionStatus {
  if (value === "valid" || value === "invalid") return value;
  throw contractError("unsupported Skill projection status");
}

function safeInteger(value: unknown, label: string): number {
  if (
    typeof value !== "number" || !Number.isSafeInteger(value) || value < 0 ||
    value > SKILL_API_LIMITS.maxSafeInteger
  ) {
    throw contractError(`${label} must be a non-negative safe integer`);
  }
  return value;
}

function boundedArray(
  value: unknown,
  label: string,
  limit: number,
): unknown[] {
  if (!Array.isArray(value) || value.length > limit) {
    throw contractError(`${label} must be a bounded array`);
  }
  return value;
}

function optionalBoundedString(
  value: unknown,
  label: string,
  limit: number,
): string | undefined {
  return value === undefined
    ? undefined
    : boundedString(value, label, limit, false);
}

function boundedString(
  value: unknown,
  label: string,
  limit: number,
  allowEmpty: boolean,
): string {
  if (
    typeof value !== "string" || (!allowEmpty && value.length === 0) ||
    new TextEncoder().encode(value).length > limit
  ) {
    throw contractError(`${label} must be a bounded string`);
  }
  return value;
}

function isVirtualPath(value: string): boolean {
  return !value.startsWith("/") && !value.includes("\\") &&
    value.split("/").every((part) =>
      part !== "" && part !== "." && part !== ".."
    );
}

function strictObject(
  value: unknown,
  allowedKeys: readonly string[],
  label: string,
  optionalKeys: readonly string[] = [],
): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw contractError(`${label} must be an object`);
  }
  const record = value as Record<string, unknown>;
  const allowed = new Set(allowedKeys);
  if (Object.keys(record).some((key) => !allowed.has(key))) {
    throw contractError(`${label} contains unknown fields`);
  }
  const optional = new Set(optionalKeys);
  if (allowedKeys.some((key) => !optional.has(key) && !(key in record))) {
    throw contractError(`${label} is missing required fields`);
  }
  return record;
}

function contractError(message: string): SkillApiContractError {
  return new SkillApiContractError(message.slice(0, 256));
}
