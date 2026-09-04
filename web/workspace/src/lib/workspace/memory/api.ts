import type {
  Diagnostic,
  DiagnosticSeverity,
  MemoryCandidateKind,
  MemoryDocumentResponse,
  MemoryEvidenceOrigin,
  MemoryEvidenceOriginKind,
  MemorySourceEvidenceRef,
  MemorySourceRef,
  MemoryStagingEntry,
  MemoryStagingEvidence,
  MemoryStagingListResponse,
  MemoryStagingRecord,
} from "$lib/generated/memory-api";

const MAX_STAGING_ITEMS = 500;
const MAX_EVIDENCE_PER_RECORD = 500;
const MAX_SOURCE_REFS_PER_RECORD = 500;
const MAX_ORIGIN_VALUE_LENGTH = 512;

const candidateKinds = new Set<MemoryCandidateKind>([
  "preference",
  "working_assumption",
  "constraint",
  "decision",
  "open_question",
  "lesson",
]);
const originKinds = new Set<MemoryEvidenceOriginKind>([
  "human_input",
  "worker_input",
  "flow_instruction",
  "backend_instruction",
  "model_output",
  "tool_output",
  "derived_summary",
  "legacy_unknown",
]);
const diagnosticSeverities = new Set<DiagnosticSeverity>([
  "info",
  "warning",
  "error",
]);

export function parseMemoryDocumentResponse(
  value: unknown,
): MemoryDocumentResponse {
  const record = strictRecord(
    value,
    ["body_md", "created_at", "updated_at", "bytes", "record_source"],
    "Memory document response",
  );
  return {
    body_md: requiredString(record, "body_md"),
    created_at: requiredString(record, "created_at"),
    updated_at: requiredString(record, "updated_at"),
    bytes: requiredNonNegativeInteger(record, "bytes"),
    record_source: requiredString(record, "record_source"),
  };
}

export function parseMemoryStagingListResponse(
  value: unknown,
): MemoryStagingListResponse {
  const record = strictRecord(
    value,
    [
      "limit",
      "returned_count",
      "total_valid_count",
      "invalid_count",
      "truncated",
      "order",
      "record_authority",
      "items",
      "diagnostics",
    ],
    "Memory staging list response",
  );
  const items = boundedArray(record.items, MAX_STAGING_ITEMS, "items").map(
    parseStagingEntry,
  );
  const diagnostics = boundedArray(
    record.diagnostics,
    MAX_STAGING_ITEMS,
    "diagnostics",
  ).map(parseDiagnostic);
  const returnedCount = requiredNonNegativeInteger(record, "returned_count");
  if (returnedCount !== items.length) {
    invalid("returned_count does not match items");
  }
  return {
    limit: requiredNonNegativeInteger(record, "limit"),
    returned_count: returnedCount,
    total_valid_count: requiredNonNegativeInteger(record, "total_valid_count"),
    invalid_count: requiredNonNegativeInteger(record, "invalid_count"),
    truncated: requiredBoolean(record, "truncated"),
    order: requiredString(record, "order"),
    record_authority: requiredString(record, "record_authority"),
    items,
    diagnostics,
  };
}

function parseStagingEntry(value: unknown): MemoryStagingEntry {
  const record = strictRecord(
    value,
    ["id", "byte_len", "record"],
    "Memory staging entry",
  );
  return {
    id: requiredString(record, "id"),
    byte_len: requiredNonNegativeInteger(record, "byte_len"),
    record: parseStagingRecord(record.record),
  };
}

function parseStagingRecord(value: unknown): MemoryStagingRecord {
  const record = strictRecord(
    value,
    [
      "schema_version",
      "id",
      "extract_run_id",
      "source",
      "kind",
      "claim",
      "why_useful",
      "staleness",
      "evidence",
      "source_refs",
    ],
    "Memory staging record",
  );
  const kind = requiredString(record, "kind") as MemoryCandidateKind;
  if (!candidateKinds.has(kind)) {
    invalid("unknown Memory candidate kind");
  }
  return {
    schema_version: requiredNonNegativeInteger(record, "schema_version"),
    id: requiredString(record, "id"),
    extract_run_id: requiredString(record, "extract_run_id"),
    source: parseSourceRef(record.source),
    kind,
    claim: requiredString(record, "claim"),
    why_useful: requiredString(record, "why_useful"),
    staleness: nullableString(record, "staleness"),
    evidence: boundedArray(
      record.evidence,
      MAX_EVIDENCE_PER_RECORD,
      "evidence",
    ).map(parseStagingEvidence),
    source_refs: boundedArray(
      record.source_refs,
      MAX_SOURCE_REFS_PER_RECORD,
      "source_refs",
    ).map(parseSourceEvidenceRef),
  };
}

function parseSourceRef(value: unknown): MemorySourceRef {
  const record = strictRecord(
    value,
    ["segment_id", "range"],
    "Memory source ref",
  );
  return {
    segment_id: requiredString(record, "segment_id"),
    range: parseEntryRange(record.range, "range"),
  };
}

function parseStagingEvidence(value: unknown): MemoryStagingEvidence {
  const record = strictRecord(
    value,
    ["id", "kind", "entry_range", "origin", "excerpt", "summary"],
    "Memory staging evidence",
    ["origin"],
  );
  const result: MemoryStagingEvidence = {
    id: requiredString(record, "id"),
    kind: requiredString(record, "kind"),
    entry_range: parseNullableEntryRange(record.entry_range, "entry_range"),
    excerpt: nullableString(record, "excerpt"),
    summary: nullableString(record, "summary"),
  };
  if ("origin" in record) {
    result.origin = record.origin === null
      ? null
      : parseEvidenceOrigin(record.origin);
  }
  return result;
}

function parseSourceEvidenceRef(value: unknown): MemorySourceEvidenceRef {
  const record = strictRecord(
    value,
    [
      "session_id",
      "segment_id",
      "entry_range",
      "evidence_id",
      "origin",
      "evidence_kind",
      "label",
      "summary",
    ],
    "Memory source evidence ref",
    ["origin"],
  );
  const result: MemorySourceEvidenceRef = {
    session_id: nullableString(record, "session_id"),
    segment_id: nullableString(record, "segment_id"),
    entry_range: parseNullableEntryRange(record.entry_range, "entry_range"),
    evidence_id: nullableString(record, "evidence_id"),
    evidence_kind: nullableString(record, "evidence_kind"),
    label: nullableString(record, "label"),
    summary: nullableString(record, "summary"),
  };
  if ("origin" in record) {
    result.origin = record.origin === null
      ? null
      : parseEvidenceOrigin(record.origin);
  }
  return result;
}

function parseEvidenceOrigin(value: unknown): MemoryEvidenceOrigin {
  const optional = [
    "account_id",
    "workspace_id",
    "runtime_id",
    "worker_id",
    "flow_selector",
    "flow_definition_id",
    "flow_definition_revision",
  ] as const;
  const record = strictRecord(
    value,
    ["kind", ...optional],
    "Memory evidence origin",
    [...optional],
  );
  const kind = requiredString(record, "kind") as MemoryEvidenceOriginKind;
  if (!originKinds.has(kind)) {
    invalid("unknown Memory evidence origin kind");
  }
  const result: MemoryEvidenceOrigin = { kind };
  for (
    const key of [
      "account_id",
      "workspace_id",
      "runtime_id",
      "worker_id",
      "flow_selector",
      "flow_definition_id",
    ] as const
  ) {
    if (key in record) {
      const text = nullableString(record, key);
      if (text !== null && text.length > MAX_ORIGIN_VALUE_LENGTH) {
        invalid(`${key} exceeds the Memory origin limit`);
      }
      result[key] = text;
    }
  }
  if ("flow_definition_revision" in record) {
    result.flow_definition_revision = record.flow_definition_revision === null
      ? null
      : nonNegativeInteger(
        record.flow_definition_revision,
        "flow_definition_revision",
      );
  }
  return result;
}

function parseDiagnostic(value: unknown): Diagnostic {
  const record = strictRecord(
    value,
    ["code", "severity", "message"],
    "Memory diagnostic",
  );
  const severity = requiredString(record, "severity") as DiagnosticSeverity;
  if (!diagnosticSeverities.has(severity)) {
    invalid("unknown diagnostic severity");
  }
  return {
    code: requiredString(record, "code"),
    severity,
    message: requiredString(record, "message"),
  };
}

function strictRecord(
  value: unknown,
  keys: readonly string[],
  label: string,
  optionalKeys: readonly string[] = [],
): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    invalid(`${label} must be an object`);
  }
  const record = value as Record<string, unknown>;
  const allowed = new Set(keys);
  for (const key of Object.keys(record)) {
    if (!allowed.has(key)) {
      invalid(`${label} has an unknown field`);
    }
  }
  const optional = new Set(optionalKeys);
  for (const key of keys) {
    if (!optional.has(key) && !(key in record)) {
      invalid(`${label} is missing a required field`);
    }
  }
  return record;
}

function boundedArray(
  value: unknown,
  maximum: number,
  label: string,
): unknown[] {
  if (!Array.isArray(value) || value.length > maximum) {
    invalid(`${label} must be a bounded array`);
  }
  return value;
}

function requiredString(record: Record<string, unknown>, key: string): string {
  if (typeof record[key] !== "string") {
    invalid(`${key} must be a string`);
  }
  return record[key];
}

function nullableString(
  record: Record<string, unknown>,
  key: string,
): string | null {
  const value = record[key];
  if (value !== null && typeof value !== "string") {
    invalid(`${key} must be a string or null`);
  }
  return value;
}

function requiredBoolean(
  record: Record<string, unknown>,
  key: string,
): boolean {
  if (typeof record[key] !== "boolean") {
    invalid(`${key} must be a boolean`);
  }
  return record[key];
}

function requiredNonNegativeInteger(
  record: Record<string, unknown>,
  key: string,
): number {
  return nonNegativeInteger(record[key], key);
}

function nonNegativeInteger(value: unknown, label: string): number {
  if (!Number.isSafeInteger(value) || (value as number) < 0) {
    invalid(`${label} must be a non-negative safe integer`);
  }
  return value as number;
}

function parseEntryRange(value: unknown, label: string): [number, number] {
  if (!Array.isArray(value) || value.length !== 2) {
    invalid(`${label} must be a two-item entry range`);
  }
  return [
    nonNegativeInteger(value[0], label),
    nonNegativeInteger(value[1], label),
  ];
}

function parseNullableEntryRange(
  value: unknown,
  label: string,
): [number, number] | null {
  return value === null ? null : parseEntryRange(value, label);
}

function invalid(message: string): never {
  throw new Error(`Invalid Memory API response: ${message}`);
}
