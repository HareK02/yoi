import { MEMORY_API_LIMITS } from "#lib/generated/memory-api.ts";
import { readBoundedJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
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
  SubjektivMemoryEvidence,
  SubjektivMemoryEvidenceCandidate,
  SubjektivMemoryListRevisionsResponse,
  SubjektivMemoryQueryItem,
  SubjektivMemoryQueryResponse,
  SubjektivMemoryReadResponse,
  SubjektivMemoryRevisionItem,
  SubjektivMemoryRevisionRef,
  SubjektivMemorySourceEvidenceRef,
  SubjektivMemoryState,
  SubjektivResidentSurfaceAvailability,
  SubjektivResidentSurfaceResponse,
  SubjektivResidentSurfaceSnapshot,
  SubjektivSubjectCreateRequest,
  SubjektivSubjectListResponse,
  SubjektivSubjectResponse,
  SubjektivSubjectState,
} from "#lib/generated/memory-api.ts";
import { parseWorkerLaunchWorkerSummary } from "#lib/workspace/api/workers.ts";

const MAX_STAGING_ITEMS = MEMORY_API_LIMITS.maxCollectionItems;
const MAX_EVIDENCE_PER_RECORD = MEMORY_API_LIMITS.maxCollectionItems;
const MAX_SOURCE_REFS_PER_RECORD = MEMORY_API_LIMITS.maxCollectionItems;
const MAX_ORIGIN_VALUE_LENGTH = MEMORY_API_LIMITS.maxIdentifierBytes;

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
const subjectStates = new Set<SubjektivSubjectState>(["active", "retired"]);
const surfaceAvailabilities = new Set<SubjektivResidentSurfaceAvailability>([
  "ungenerated",
  "stale",
  "failed",
  "ready",
]);
const memoryStates = new Set<SubjektivMemoryState>([
  "active",
  "resolved",
  "retracted",
]);
const MAX_SUBJECTS = 100;
const MAX_CURRENT_MEMORIES = 100;
const MAX_REVISIONS = 100;
const MAX_MEMORY_REFS = MEMORY_API_LIMITS.maxCollectionItems;

export const MEMORY_API_LOAD_POLICY = {
  diagnosticLabel: "Memory API",
  maxResponseBytes: MEMORY_API_LIMITS.maxResponseBytes,
} as const;

const MAX_SUBJECT_ROLE_BYTES = 256;
const MAX_SUBJECT_CREATE_ERROR_BYTES = 1024 * 1024;

export type SubjektivSubjectCreateErrorKind =
  | "validation"
  | "rejected"
  | "unknown_outcome";

export class SubjektivSubjectCreateError extends Error {
  readonly kind: SubjektivSubjectCreateErrorKind;
  readonly status: number | null;

  constructor(
    kind: SubjektivSubjectCreateErrorKind,
    message: string,
    status: number | null = null,
  ) {
    super(message);
    this.name = "SubjektivSubjectCreateError";
    this.kind = kind;
    this.status = status;
  }
}

export function validateSubjektivSubjectCreateRequest(
  request: SubjektivSubjectCreateRequest,
): SubjektivSubjectCreateRequest {
  if (request.role.trim().length === 0) {
    throw new SubjektivSubjectCreateError(
      "validation",
      "Subject role must not be empty.",
    );
  }
  if (
    new TextEncoder().encode(request.role).byteLength >
      MAX_SUBJECT_ROLE_BYTES ||
    /\p{Cc}/u.test(request.role)
  ) {
    throw new SubjektivSubjectCreateError(
      "validation",
      "Subject role must be at most 256 bytes and contain no control characters.",
    );
  }
  return { role: request.role };
}

export async function createSubjektivSubject(
  fetchFn: typeof fetch,
  workspaceId: string,
  request: SubjektivSubjectCreateRequest,
): Promise<SubjektivSubjectResponse> {
  const validated = validateSubjektivSubjectCreateRequest(request);
  const path = workspaceApiPath(workspaceId, "/subjektiv/subjects");
  let response: Response;
  try {
    response = await fetchFn(path, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(validated),
    });
  } catch {
    throw new SubjektivSubjectCreateError(
      "unknown_outcome",
      "The request outcome is unknown. A Subject may have been created.",
    );
  }

  if (!response.ok) {
    const message = await subjektivSubjectCreateErrorMessage(response);
    if (response.status >= 400 && response.status < 500) {
      throw new SubjektivSubjectCreateError(
        "rejected",
        message,
        response.status,
      );
    }
    throw new SubjektivSubjectCreateError(
      "unknown_outcome",
      "The request outcome is unknown. A Subject may have been created.",
      response.status,
    );
  }

  try {
    const subject = parseSubjektivSubjectResponse(
      await readBoundedJson(response, MEMORY_API_LIMITS.maxResponseBytes),
    );
    if (subject.role !== validated.role) {
      throw new Error("created Subject role does not match the request");
    }
    return subject;
  } catch {
    throw new SubjektivSubjectCreateError(
      "unknown_outcome",
      "The Subject may have been created, but the response could not be confirmed.",
    );
  }
}

async function subjektivSubjectCreateErrorMessage(
  response: Response,
): Promise<string> {
  try {
    const payload = await readBoundedJson(
      response,
      MAX_SUBJECT_CREATE_ERROR_BYTES,
    );
    if (typeof payload === "object" && payload !== null) {
      const record = payload as Record<string, unknown>;
      if (typeof record.message === "string" && record.message.length > 0) {
        return record.message;
      }
      if (typeof record.error === "string" && record.error.length > 0) {
        return record.error;
      }
    }
  } catch {
    // A rejected request still has a known outcome even when its body is invalid.
  }
  return `Subject creation failed with HTTP ${response.status}.`;
}

export function parseMemoryDocumentResponse(
  value: unknown,
): MemoryDocumentResponse {
  const record = strictRecord(
    value,
    ["body_md", "created_at", "updated_at", "bytes", "record_source"],
    "Memory document response",
  );
  return {
    body_md: requiredString(
      record,
      "body_md",
      MEMORY_API_LIMITS.maxDocumentBytes,
    ),
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
      if (
        text !== null &&
        new TextEncoder().encode(text).byteLength > MAX_ORIGIN_VALUE_LENGTH
      ) {
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

export function parseSubjektivSubjectResponse(
  value: unknown,
  expectedSubjectId?: string,
): SubjektivSubjectResponse {
  const record = strictRecord(
    value,
    [
      "id",
      "role",
      "state",
      "store_revision",
      "created_at",
      "updated_at",
      "current_worker",
    ],
    "Subject response",
    ["current_worker"],
  );
  const id = requiredIdentifier(record, "id");
  if (expectedSubjectId !== undefined && id !== expectedSubjectId) {
    invalid("Subject response identity does not match the requested subject");
  }
  const state = requiredString(record, "state") as SubjektivSubjectState;
  if (!subjectStates.has(state)) invalid("unknown Subject state");
  const result: SubjektivSubjectResponse = {
    id,
    role: requiredString(record, "role"),
    state,
    store_revision: requiredNonNegativeInteger(record, "store_revision"),
    created_at: requiredString(record, "created_at"),
    updated_at: requiredString(record, "updated_at"),
  };
  if ("current_worker" in record) {
    result.current_worker = record.current_worker === null
      ? null
      : parseWorkerLaunchWorkerSummary(
        record.current_worker,
        "Subject current worker",
      );
  }
  return result;
}

export function parseSubjektivSubjectListResponse(
  value: unknown,
): SubjektivSubjectListResponse {
  const record = strictRecord(
    value,
    ["limit", "items", "next_cursor", "has_more"],
    "Subject list response",
    ["next_cursor"],
  );
  const limit = positiveInteger(record.limit, "limit");
  if (limit > MAX_SUBJECTS) invalid("limit exceeds the Subject list bound");
  const items = boundedArray(record.items, MAX_SUBJECTS, "items").map((item) =>
    parseSubjektivSubjectResponse(item)
  );
  if (items.length > limit) invalid("items exceeds the declared Subject limit");
  assertUnique(items.map((item) => item.id), "Subject ids");
  const hasMore = requiredBoolean(record, "has_more");
  const nextCursor = optionalNullableString(record, "next_cursor");
  assertCursorInvariant(hasMore, nextCursor, "Subject list");
  const result: SubjektivSubjectListResponse = {
    limit,
    items,
    has_more: hasMore,
  };
  if (nextCursor !== undefined) result.next_cursor = nextCursor;
  return result;
}

export function parseSubjektivResidentSurfaceResponse(
  value: unknown,
  expectedSubjectId?: string,
): SubjektivResidentSurfaceResponse {
  const record = strictRecord(
    value,
    ["subject_id", "availability", "snapshot"],
    "Resident surface response",
    ["snapshot"],
  );
  const subjectId = requiredIdentifier(record, "subject_id");
  if (expectedSubjectId !== undefined && subjectId !== expectedSubjectId) {
    invalid("Resident surface identity does not match the requested subject");
  }
  const availability = requiredString(
    record,
    "availability",
  ) as SubjektivResidentSurfaceAvailability;
  if (!surfaceAvailabilities.has(availability)) {
    invalid("unknown resident surface availability");
  }
  const snapshot = "snapshot" in record && record.snapshot !== null
    ? parseResidentSurfaceSnapshot(record.snapshot)
    : null;
  if (availability === "ready" && snapshot === null) {
    invalid("ready resident surface requires a snapshot");
  }
  if (availability !== "ready" && snapshot !== null) {
    invalid("non-ready resident surface must not include a snapshot");
  }
  const result: SubjektivResidentSurfaceResponse = {
    subject_id: subjectId,
    availability,
  };
  if (snapshot !== null) result.snapshot = snapshot;
  return result;
}

function parseResidentSurfaceSnapshot(
  value: unknown,
): SubjektivResidentSurfaceSnapshot {
  const record = strictRecord(
    value,
    [
      "snapshot_id",
      "body_md",
      "memory_refs",
      "built_from_store_revision",
      "created_at",
    ],
    "Resident surface snapshot",
  );
  return {
    snapshot_id: requiredIdentifier(record, "snapshot_id"),
    body_md: requiredString(
      record,
      "body_md",
      MEMORY_API_LIMITS.maxDocumentBytes,
    ),
    memory_refs: boundedArray(
      record.memory_refs,
      MAX_MEMORY_REFS,
      "memory_refs",
    ).map(parseMemoryRevisionRef),
    built_from_store_revision: requiredNonNegativeInteger(
      record,
      "built_from_store_revision",
    ),
    created_at: requiredString(record, "created_at"),
  };
}

export function parseSubjektivMemoryQueryResponse(
  value: unknown,
): SubjektivMemoryQueryResponse {
  const record = strictRecord(
    value,
    ["items", "next_cursor", "has_more"],
    "Current Memory list response",
    ["next_cursor"],
  );
  const items = boundedArray(
    record.items,
    MAX_CURRENT_MEMORIES,
    "items",
  ).map(parseMemoryQueryItem);
  assertUnique(items.map((item) => item.id), "Memory ids");
  const hasMore = requiredBoolean(record, "has_more");
  const nextCursor = optionalNullableString(record, "next_cursor");
  assertCursorInvariant(hasMore, nextCursor, "Memory list");
  const result: SubjektivMemoryQueryResponse = { items, has_more: hasMore };
  if (nextCursor !== undefined) result.next_cursor = nextCursor;
  return result;
}

function parseMemoryQueryItem(value: unknown): SubjektivMemoryQueryItem {
  const record = strictRecord(
    value,
    ["id", "revision", "kind", "state", "claim", "excerpt", "updated_at"],
    "Current Memory item",
  );
  return {
    id: requiredIdentifier(record, "id"),
    revision: requiredPositiveInteger(record, "revision"),
    kind: parseCandidateKind(record.kind),
    state: parseMemoryState(record.state),
    claim: requiredString(record, "claim"),
    excerpt: requiredString(record, "excerpt"),
    updated_at: requiredString(record, "updated_at"),
  };
}

export function parseSubjektivMemoryReadResponse(
  value: unknown,
  expectedMemoryId?: string,
): SubjektivMemoryReadResponse {
  const record = strictRecord(
    value,
    [
      "memory_id",
      "revision",
      "current_revision",
      "kind",
      "state",
      "claim",
      "body_md",
      "why_useful",
      "staleness",
      "change_reason",
      "created_at",
      "updated_at",
      "body_offset",
      "body_byte_offset",
      "body_next_offset",
      "body_next_byte_offset",
      "body_truncated",
      "source_candidate_ids",
      "source_candidates",
      "derived_from",
      "evidence_next_cursor",
      "evidence_has_more",
    ],
    "Memory detail response",
    [
      "staleness",
      "body_next_offset",
      "body_next_byte_offset",
      "evidence_next_cursor",
    ],
  );
  const memoryId = requiredIdentifier(record, "memory_id");
  if (expectedMemoryId !== undefined && memoryId !== expectedMemoryId) {
    invalid("Memory detail identity does not match the requested Memory");
  }
  const revision = requiredPositiveInteger(record, "revision");
  const currentRevision = requiredPositiveInteger(record, "current_revision");
  if (revision > currentRevision) invalid("revision exceeds current_revision");
  const bodyTruncated = requiredBoolean(record, "body_truncated");
  const bodyNextOffset = optionalNonNegativeInteger(record, "body_next_offset");
  const bodyNextByteOffset = optionalNonNegativeInteger(
    record,
    "body_next_byte_offset",
  );
  if (
    bodyTruncated !==
      (bodyNextOffset !== undefined && bodyNextByteOffset !== undefined)
  ) {
    invalid("body continuation fields do not match body_truncated");
  }
  const sourceCandidateIds = boundedArray(
    record.source_candidate_ids,
    MAX_MEMORY_REFS,
    "source_candidate_ids",
  ).map((item) => identifier(item, "source_candidate_ids item"));
  assertUnique(sourceCandidateIds, "source candidate ids");
  const sourceCandidates = boundedArray(
    record.source_candidates,
    MAX_MEMORY_REFS,
    "source_candidates",
  ).map(parseMemoryEvidenceCandidate);
  if (
    sourceCandidateIds.length !== sourceCandidates.length ||
    sourceCandidateIds.some((id, index) =>
      sourceCandidates[index]?.candidate_id !== id
    )
  ) {
    invalid("source candidate ids do not match source candidates");
  }
  const evidenceHasMore = requiredBoolean(record, "evidence_has_more");
  const evidenceNextCursor = optionalNullableString(
    record,
    "evidence_next_cursor",
  );
  assertCursorInvariant(evidenceHasMore, evidenceNextCursor, "Memory evidence");
  const result: SubjektivMemoryReadResponse = {
    memory_id: memoryId,
    revision,
    current_revision: currentRevision,
    kind: parseCandidateKind(record.kind),
    state: parseMemoryState(record.state),
    claim: requiredString(record, "claim"),
    body_md: requiredString(
      record,
      "body_md",
      MEMORY_API_LIMITS.maxDocumentBytes,
    ),
    why_useful: requiredString(record, "why_useful"),
    change_reason: requiredString(record, "change_reason"),
    created_at: requiredString(record, "created_at"),
    updated_at: requiredString(record, "updated_at"),
    body_offset: requiredNonNegativeInteger(record, "body_offset"),
    body_byte_offset: requiredNonNegativeInteger(record, "body_byte_offset"),
    body_truncated: bodyTruncated,
    source_candidate_ids: sourceCandidateIds,
    source_candidates: sourceCandidates,
    derived_from: boundedArray(
      record.derived_from,
      MAX_MEMORY_REFS,
      "derived_from",
    ).map(parseMemoryRevisionRef),
    evidence_has_more: evidenceHasMore,
  };
  if ("staleness" in record) {
    result.staleness = nullableString(record, "staleness");
  }
  if (bodyNextOffset !== undefined) result.body_next_offset = bodyNextOffset;
  if (bodyNextByteOffset !== undefined) {
    result.body_next_byte_offset = bodyNextByteOffset;
  }
  if (evidenceNextCursor !== undefined) {
    result.evidence_next_cursor = evidenceNextCursor;
  }
  return result;
}

function parseMemoryEvidenceCandidate(
  value: unknown,
): SubjektivMemoryEvidenceCandidate {
  const record = strictRecord(
    value,
    [
      "candidate_id",
      "evidence",
      "evidence_total",
      "evidence_truncated",
      "source_refs",
      "source_refs_total",
      "source_refs_truncated",
    ],
    "Memory evidence candidate",
  );
  const evidence = boundedArray(
    record.evidence,
    MAX_EVIDENCE_PER_RECORD,
    "evidence",
  ).map(parseMemoryEvidence);
  assertUnique(evidence.map((item) => item.id), "evidence ids");
  const evidenceTotal = requiredNonNegativeInteger(record, "evidence_total");
  const evidenceTruncated = requiredBoolean(record, "evidence_truncated");
  assertCollectionTotal(
    evidence.length,
    evidenceTotal,
    evidenceTruncated,
    "evidence",
  );
  const sourceRefs = boundedArray(
    record.source_refs,
    MAX_SOURCE_REFS_PER_RECORD,
    "source_refs",
  ).map(parseMemorySourceEvidenceRef);
  const sourceRefsTotal = requiredNonNegativeInteger(
    record,
    "source_refs_total",
  );
  const sourceRefsTruncated = requiredBoolean(
    record,
    "source_refs_truncated",
  );
  assertCollectionTotal(
    sourceRefs.length,
    sourceRefsTotal,
    sourceRefsTruncated,
    "source refs",
  );
  return {
    candidate_id: requiredIdentifier(record, "candidate_id"),
    evidence,
    evidence_total: evidenceTotal,
    evidence_truncated: evidenceTruncated,
    source_refs: sourceRefs,
    source_refs_total: sourceRefsTotal,
    source_refs_truncated: sourceRefsTruncated,
  };
}

function parseMemoryEvidence(value: unknown): SubjektivMemoryEvidence {
  const record = strictRecord(
    value,
    ["id", "kind", "entry_range", "origin", "excerpt", "summary"],
    "Memory evidence",
    ["entry_range", "origin", "excerpt", "summary"],
  );
  const result: SubjektivMemoryEvidence = {
    id: requiredIdentifier(record, "id"),
    kind: requiredString(record, "kind"),
  };
  if ("entry_range" in record) {
    result.entry_range = record.entry_range === null
      ? null
      : parseEntryRange(record.entry_range, "entry_range");
  }
  if ("origin" in record) {
    result.origin = record.origin === null
      ? null
      : parseEvidenceOrigin(record.origin);
  }
  if ("excerpt" in record) result.excerpt = nullableString(record, "excerpt");
  if ("summary" in record) result.summary = nullableString(record, "summary");
  return result;
}

function parseMemorySourceEvidenceRef(
  value: unknown,
): SubjektivMemorySourceEvidenceRef {
  const keys = [
    "session_id",
    "segment_id",
    "entry_range",
    "evidence_id",
    "origin",
    "evidence_kind",
    "label",
    "summary",
  ] as const;
  const record = strictRecord(
    value,
    keys,
    "Memory source evidence ref",
    keys,
  );
  const result: SubjektivMemorySourceEvidenceRef = {};
  for (
    const key of [
      "session_id",
      "segment_id",
      "evidence_id",
      "evidence_kind",
      "label",
      "summary",
    ] as const
  ) {
    if (key in record) result[key] = nullableString(record, key);
  }
  if ("entry_range" in record) {
    result.entry_range = record.entry_range === null
      ? null
      : parseEntryRange(record.entry_range, "entry_range");
  }
  if ("origin" in record) {
    result.origin = record.origin === null
      ? null
      : parseEvidenceOrigin(record.origin);
  }
  return result;
}

export function parseSubjektivMemoryListRevisionsResponse(
  value: unknown,
  expectedMemoryId?: string,
): SubjektivMemoryListRevisionsResponse {
  const record = strictRecord(
    value,
    ["memory_id", "current_revision", "items", "next_cursor", "has_more"],
    "Memory revisions response",
    ["next_cursor"],
  );
  const memoryId = requiredIdentifier(record, "memory_id");
  if (expectedMemoryId !== undefined && memoryId !== expectedMemoryId) {
    invalid("Memory revisions identity does not match the requested Memory");
  }
  const currentRevision = requiredPositiveInteger(record, "current_revision");
  const items = boundedArray(record.items, MAX_REVISIONS, "items").map(
    parseMemoryRevisionItem,
  );
  if (items.some((item) => item.revision > currentRevision)) {
    invalid("revision history contains a future revision");
  }
  assertUnique(
    items.map((item) => String(item.revision)),
    "Memory revisions",
  );
  const hasMore = requiredBoolean(record, "has_more");
  const nextCursor = optionalNullableString(record, "next_cursor");
  assertCursorInvariant(hasMore, nextCursor, "Memory revisions");
  const result: SubjektivMemoryListRevisionsResponse = {
    memory_id: memoryId,
    current_revision: currentRevision,
    items,
    has_more: hasMore,
  };
  if (nextCursor !== undefined) result.next_cursor = nextCursor;
  return result;
}

function parseMemoryRevisionItem(value: unknown): SubjektivMemoryRevisionItem {
  const record = strictRecord(
    value,
    ["revision", "kind", "state", "claim", "change_reason", "updated_at"],
    "Memory revision item",
  );
  return {
    revision: requiredPositiveInteger(record, "revision"),
    kind: parseCandidateKind(record.kind),
    state: parseMemoryState(record.state),
    claim: requiredString(record, "claim"),
    change_reason: requiredString(record, "change_reason"),
    updated_at: requiredString(record, "updated_at"),
  };
}

function parseMemoryRevisionRef(value: unknown): SubjektivMemoryRevisionRef {
  const record = strictRecord(
    value,
    ["memory_id", "revision"],
    "Memory revision ref",
  );
  return {
    memory_id: requiredIdentifier(record, "memory_id"),
    revision: requiredPositiveInteger(record, "revision"),
  };
}

function parseCandidateKind(value: unknown): MemoryCandidateKind {
  if (
    typeof value !== "string" ||
    !candidateKinds.has(value as MemoryCandidateKind)
  ) {
    invalid("unknown Memory candidate kind");
  }
  return value as MemoryCandidateKind;
}

function parseMemoryState(value: unknown): SubjektivMemoryState {
  if (
    typeof value !== "string" ||
    !memoryStates.has(value as SubjektivMemoryState)
  ) {
    invalid("unknown Memory state");
  }
  return value as SubjektivMemoryState;
}

function assertCollectionTotal(
  returned: number,
  total: number,
  truncated: boolean,
  label: string,
): void {
  if (total < returned || truncated !== (total > returned)) {
    invalid(`${label} total does not match its truncated state`);
  }
}

function assertCursorInvariant(
  hasMore: boolean,
  cursor: string | null | undefined,
  label: string,
): void {
  if (hasMore !== (typeof cursor === "string" && cursor.length > 0)) {
    invalid(`${label} cursor does not match has_more`);
  }
}

function assertUnique(values: string[], label: string): void {
  if (new Set(values).size !== values.length) {
    invalid(`${label} must be unique`);
  }
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

function requiredString(
  record: Record<string, unknown>,
  key: string,
  maxBytes: number = MEMORY_API_LIMITS.maxStringBytes,
): string {
  const value = record[key];
  if (
    typeof value !== "string" ||
    new TextEncoder().encode(value).byteLength > maxBytes
  ) {
    invalid(`${key} must be a bounded string`);
  }
  return value;
}

function nullableString(
  record: Record<string, unknown>,
  key: string,
): string | null {
  const value = record[key];
  if (
    value !== null &&
    (typeof value !== "string" ||
      new TextEncoder().encode(value).byteLength >
        MEMORY_API_LIMITS.maxStringBytes)
  ) {
    invalid(`${key} must be a bounded string or null`);
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

function requiredPositiveInteger(
  record: Record<string, unknown>,
  key: string,
): number {
  return positiveInteger(record[key], key);
}

function positiveInteger(value: unknown, label: string): number {
  const result = nonNegativeInteger(value, label);
  if (result === 0) invalid(`${label} must be a positive safe integer`);
  return result;
}

function optionalNonNegativeInteger(
  record: Record<string, unknown>,
  key: string,
): number | undefined {
  if (!(key in record)) return undefined;
  if (record[key] === null) return undefined;
  return nonNegativeInteger(record[key], key);
}

function requiredIdentifier(
  record: Record<string, unknown>,
  key: string,
): string {
  return identifier(record[key], key);
}

function identifier(value: unknown, label: string): string {
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    new TextEncoder().encode(value).byteLength >
      MEMORY_API_LIMITS.maxIdentifierBytes
  ) {
    invalid(`${label} must be a non-empty bounded identifier`);
  }
  return value;
}

function optionalNullableString(
  record: Record<string, unknown>,
  key: string,
): string | null | undefined {
  if (!(key in record)) return undefined;
  return nullableString(record, key);
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
  const start = nonNegativeInteger(value[0], label);
  const end = nonNegativeInteger(value[1], label);
  if (start > end) invalid(`${label} must be ordered`);
  return [start, end];
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
