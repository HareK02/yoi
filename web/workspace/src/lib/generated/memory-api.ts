// Generated from server-api. Do not edit by hand.
// Regenerate: cargo run -q -p server-api --features typescript --example generate_memory_api_types > web/workspace/src/lib/generated/memory-api.ts

export const MEMORY_API_LIMITS = {
  maxResponseBytes: 8388608,
  maxDocumentBytes: 4194304,
  maxCollectionItems: 500,
  maxStringBytes: 1048576,
  maxIdentifierBytes: 512,
} as const;

export const SUBJEKTIV_API_LIMITS = {
  maxBehaviorBytes: 16384,
} as const;

export type DiagnosticSeverity = "info" | "warning" | "error";

export type Diagnostic = { code: string, severity: DiagnosticSeverity, message: string, };

export type MemoryDocumentResponse = { body_md: string, created_at: string, updated_at: string, bytes: number, record_source: string, };

export type MemoryCandidateKind = "preference" | "working_assumption" | "constraint" | "decision" | "open_question" | "lesson";

export type MemoryEvidenceOriginKind = "human_input" | "worker_input" | "flow_instruction" | "backend_instruction" | "model_output" | "tool_output" | "derived_summary" | "legacy_unknown";

export type MemoryEvidenceOrigin = { kind: MemoryEvidenceOriginKind, account_id?: string | null, workspace_id?: string | null, runtime_id?: string | null, worker_id?: string | null, flow_selector?: string | null, flow_definition_id?: string | null, flow_definition_revision?: number | null, };

export type MemorySourceRef = { segment_id: string, range: [number, number], };

export type MemoryStagingEvidence = { id: string, kind: string, entry_range: [number, number] | null, origin?: MemoryEvidenceOrigin | null, excerpt: string | null, summary: string | null, };

export type MemorySourceEvidenceRef = { session_id: string | null, segment_id: string | null, entry_range: [number, number] | null, evidence_id: string | null, origin?: MemoryEvidenceOrigin | null, evidence_kind: string | null, label: string | null, summary: string | null, };

export type MemoryStagingRecord = { schema_version: number, id: string, extract_run_id: string, source: MemorySourceRef, kind: MemoryCandidateKind, claim: string, why_useful: string, staleness: string | null, evidence: Array<MemoryStagingEvidence>, source_refs: Array<MemorySourceEvidenceRef>, };

export type MemoryStagingEntry = { id: string, byte_len: number, record: MemoryStagingRecord, };

export type MemoryStagingListResponse = { limit: number, returned_count: number, total_valid_count: number, invalid_count: number, truncated: boolean, order: string, record_authority: string, items: Array<MemoryStagingEntry>, diagnostics: Array<Diagnostic>, };

export type SubjektivSubjectState = "active" | "retired";

export type SubjektivSubjectCreateRequest = { role: string, behavior_md?: string, };

export type SubjektivSubjectBehaviorUpdateRequest = { expected_behavior_revision: number, behavior_md: string, };

export type SubjektivSubjectResponse = { id: string, role: string, behavior_md: string, behavior_revision: number, state: SubjektivSubjectState, store_revision: number, created_at: string, updated_at: string, current_worker?: | import("./worker-launch-api").WorkerLaunchWorkerSummary | null, };

export type SubjektivSubjectListResponse = { limit: number, items: Array<SubjektivSubjectResponse>, next_cursor?: string | null, has_more: boolean, };

export type SubjektivMemoryState = "active" | "resolved" | "retracted";

export type SubjektivMemoryRevisionRef = { memory_id: string, revision: number, };

export type SubjektivResidentSurfaceAvailability = "ungenerated" | "stale" | "failed" | "ready";

export type SubjektivResidentSurfaceSnapshot = { snapshot_id: string, body_md: string, memory_refs: Array<SubjektivMemoryRevisionRef>, built_from_store_revision: number, created_at: string, };

export type SubjektivResidentSurfaceResponse = { subject_id: string, availability: SubjektivResidentSurfaceAvailability, snapshot?: SubjektivResidentSurfaceSnapshot | null, };

export type SubjektivMemoryQueryItem = { id: string, revision: number, kind: MemoryCandidateKind, state: SubjektivMemoryState, claim: string, excerpt: string, updated_at: string, };

export type SubjektivMemoryQueryResponse = { items: Array<SubjektivMemoryQueryItem>, next_cursor?: string | null, has_more: boolean, };

export type SubjektivMemoryEvidence = { id: string, kind: string, entry_range?: [number, number] | null, origin?: MemoryEvidenceOrigin | null, excerpt?: string | null, summary?: string | null, };

export type SubjektivMemorySourceEvidenceRef = { session_id?: string | null, segment_id?: string | null, entry_range?: [number, number] | null, evidence_id?: string | null, origin?: MemoryEvidenceOrigin | null, evidence_kind?: string | null, label?: string | null, summary?: string | null, };

export type SubjektivMemoryEvidenceCandidate = { candidate_id: string, evidence: Array<SubjektivMemoryEvidence>, evidence_total: number, evidence_truncated: boolean, source_refs: Array<SubjektivMemorySourceEvidenceRef>, source_refs_total: number, source_refs_truncated: boolean, };

export type SubjektivMemoryReadResponse = { memory_id: string, revision: number, current_revision: number, kind: MemoryCandidateKind, state: SubjektivMemoryState, claim: string, body_md: string, why_useful: string, staleness?: string | null, change_reason: string, created_at: string, updated_at: string, body_offset: number, body_byte_offset: number, body_next_offset?: number | null, body_next_byte_offset?: number | null, body_truncated: boolean, source_candidate_ids: Array<string>, source_candidates: Array<SubjektivMemoryEvidenceCandidate>, derived_from: Array<SubjektivMemoryRevisionRef>, evidence_next_cursor?: string | null, evidence_has_more: boolean, };

export type SubjektivMemoryRevisionItem = { revision: number, kind: MemoryCandidateKind, state: SubjektivMemoryState, claim: string, change_reason: string, updated_at: string, };

export type SubjektivMemoryListRevisionsResponse = { memory_id: string, current_revision: number, items: Array<SubjektivMemoryRevisionItem>, next_cursor?: string | null, has_more: boolean, };
