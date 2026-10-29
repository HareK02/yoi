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

export type MemoryEvidenceOrigin = { kind: MemoryEvidenceOriginKind, account_id?: string | null, workspace_id?: string | null, runtime_id?: string | null, worker_id?: string | null, flow_selector?: string | null, flow_definition_id?: string | null, flow_definition_fingerprint?: number | null, };

export type MemorySourceRef = { segment_id: string, range: [number, number], };

export type MemoryStagingEvidence = { id: string, kind: string, entry_range: [number, number] | null, origin?: MemoryEvidenceOrigin | null, excerpt: string | null, summary: string | null, };

export type MemorySourceEvidenceRef = { session_id: string | null, segment_id: string | null, entry_range: [number, number] | null, evidence_id: string | null, origin?: MemoryEvidenceOrigin | null, evidence_kind: string | null, label: string | null, summary: string | null, };

export type MemoryStagingRecord = { schema_version: number, id: string, extract_run_id: string, source: MemorySourceRef, kind: MemoryCandidateKind, claim: string, why_useful: string, staleness: string | null, evidence: Array<MemoryStagingEvidence>, source_refs: Array<MemorySourceEvidenceRef>, };

export type MemoryStagingEntry = { id: string, byte_len: number, record: MemoryStagingRecord, };

export type MemoryStagingListResponse = { limit: number, returned_count: number, total_valid_count: number, invalid_count: number, truncated: boolean, order: string, record_authority: string, items: Array<MemoryStagingEntry>, diagnostics: Array<Diagnostic>, };

export type PermissionResource = { "kind": "plugin_installation", workspace_id: string, installation_id: string, } | { "kind": "job", workspace_id: string, job_id: string, } | { "kind": "ticket", workspace_id: string, ticket_id: string, } | { "kind": "objective", workspace_id: string, objective_id: string, } | { "kind": "merge_request", workspace_id: string, merge_request_id: string, } | { "kind": "subject", workspace_id: string, subject_id: string, } | { "kind": "drive_entry", workspace_id: string, node_id: string, } | { "kind": "drive_request", workspace_id: string, request_id: string, } | { "kind": "repository", workspace_id: string, repository_key: string, } | { "kind": "workdir_attachment", workspace_id: string, connection_id: string, } | { "kind": "workspace_config", workspace_id: string, path: string, recursive: boolean, } | { "kind": "network", url: string, } | { "kind": "sub_worker", runtime_id: string, worker_id: string, name: string, } | { "kind": "feature" } | { "kind": "workspace", workspace_id: string, } | { "kind": "workdir", working_directory_id: string, path: string, recursive: boolean, } | { "kind": "worker_operation", workspace_id: string, } | { "kind": "worker", runtime_id: string, worker_id: string, };

export type ScopedPermission = { operation: string, resource: PermissionResource, };

export type SubjectPermissions = { change_id: string, execution: Array<ScopedPermission>, delegation: Array<ScopedPermission>, };

export type SubjectPermissionsUpdate = { operation_id: string, expected_change_id: string | null, execution: Array<ScopedPermission>, delegation: Array<ScopedPermission>, };

export type PermissionResourceKind = "plugin_installation" | "job" | "ticket" | "objective" | "merge_request" | "subject" | "drive_entry" | "drive_request" | "repository" | "workdir_attachment" | "workspace_config" | "network" | "sub_worker" | "feature" | "workspace" | "workdir" | "worker" | "worker_operation";

export type PermissionOperationAvailability = { "kind": "available" } | { "kind": "unavailable", reason: string, };

export type PermissionOperationDescriptor = { operation: string, label: string, description: string,
/**
 * Accepted management input kinds, not a claim that a target exists or is usable.
 */
resource_kinds: Array<PermissionResourceKind>, availability: PermissionOperationAvailability, };

export type SubjectPermissionsResponse = { permissions: SubjectPermissions | null, can_manage: boolean, operations: Array<PermissionOperationDescriptor>, };

export type SubjectPermissionsMutationResponse = { permissions: SubjectPermissions,
/**
 * New operations are already fenced. These are cancellation requests and
 * Backend command cleanup evidence, not proof that a Runtime turn stopped.
 */
interruptions: Array<SubjectPermissionInterruption>, };

export type SubjectPermissionInterruption = { runtime_id: string, worker_id: string, cancellation_accepted: boolean, backend_command_sessions_closed: boolean, diagnostics: Array<string>, };

export type SubjektivSubjectState = "active" | "retired";

export type SubjektivSubjectCreateRequest = { role: string, behavior_md?: string, };

export type SubjektivSubjectDeleteRequest = { operation_id: string, };

export type SubjektivSubjectDeleteResponse = { subject_id: string, operation_id: string, completed: boolean, };

export type SubjektivSubjectBehaviorUpdateRequest = { expected_behavior_md: string, behavior_md: string, };

export type SubjektivSubjectResponse = { id: string, role: string, behavior_md: string, state: SubjektivSubjectState, memory_fingerprint: string, created_at: string, updated_at: string, current_worker?: | import("./worker-launch-api").WorkerLaunchWorkerSummary | null, };

export type SubjektivSubjectListResponse = { limit: number, items: Array<SubjektivSubjectResponse>, next_cursor?: string | null, has_more: boolean, };

export type SubjektivMemoryState = "active" | "resolved" | "retracted";

export type SubjektivMemoryChangeRef = { memory_id: string, change_id: string, };

export type SubjektivResidentSurfaceAvailability = "ungenerated" | "stale" | "failed" | "ready";

export type SubjektivResidentSurfaceSnapshot = { snapshot_id: string, body_md: string, memory_refs: Array<SubjektivMemoryChangeRef>, built_from_memory_fingerprint: string, created_at: string, };

export type SubjektivResidentSurfaceResponse = { subject_id: string, availability: SubjektivResidentSurfaceAvailability, snapshot?: SubjektivResidentSurfaceSnapshot | null, };

export type SubjektivMemoryQueryItem = { id: string, change_id: string, kind: MemoryCandidateKind, state: SubjektivMemoryState, claim: string, excerpt: string, updated_at: string, };

export type SubjektivMemoryQueryResponse = { items: Array<SubjektivMemoryQueryItem>, next_cursor?: string | null, has_more: boolean, };

export type SubjektivMemoryEvidence = { id: string, kind: string, entry_range?: [number, number] | null, origin?: MemoryEvidenceOrigin | null, excerpt?: string | null, summary?: string | null, };

export type SubjektivMemorySourceEvidenceRef = { session_id?: string | null, segment_id?: string | null, entry_range?: [number, number] | null, evidence_id?: string | null, origin?: MemoryEvidenceOrigin | null, evidence_kind?: string | null, label?: string | null, summary?: string | null, };

export type SubjektivMemoryEvidenceCandidate = { candidate_id: string, evidence: Array<SubjektivMemoryEvidence>, evidence_total: number, evidence_truncated: boolean, source_refs: Array<SubjektivMemorySourceEvidenceRef>, source_refs_total: number, source_refs_truncated: boolean, };

export type SubjektivMemoryReadResponse = { memory_id: string, change_id: string, current_change_id: string, kind: MemoryCandidateKind, state: SubjektivMemoryState, claim: string, body_md: string, why_useful: string, staleness?: string | null, change_reason: string, created_at: string, updated_at: string, body_offset: number, body_byte_offset: number, body_next_offset?: number | null, body_next_byte_offset?: number | null, body_truncated: boolean, source_candidate_ids: Array<string>, source_candidates: Array<SubjektivMemoryEvidenceCandidate>, derived_from: Array<SubjektivMemoryChangeRef>, evidence_next_cursor?: string | null, evidence_has_more: boolean, };

export type SubjektivMemoryChangeItem = { change_id: string, kind: MemoryCandidateKind, state: SubjektivMemoryState, claim: string, change_reason: string, updated_at: string, };

export type SubjektivMemoryListChangesResponse = { memory_id: string, current_change_id: string, items: Array<SubjektivMemoryChangeItem>, next_cursor?: string | null, has_more: boolean, };
