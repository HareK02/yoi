// Generated from workspace-api. Do not edit by hand.
// Regenerate: cargo run -q -p workspace-api --features typescript --example generate_memory_api_types > web/workspace/src/lib/generated/memory-api.ts

export type DiagnosticSeverity = "info" | "warning" | "error";

export type Diagnostic = {
  code: string;
  severity: DiagnosticSeverity;
  message: string;
};

export type MemoryDocumentResponse = {
  body_md: string;
  created_at: string;
  updated_at: string;
  bytes: number;
  record_source: string;
};

export type MemoryCandidateKind =
  | "preference"
  | "working_assumption"
  | "constraint"
  | "decision"
  | "open_question"
  | "lesson";

export type MemoryEvidenceOriginKind =
  | "human_input"
  | "worker_input"
  | "flow_instruction"
  | "backend_instruction"
  | "model_output"
  | "tool_output"
  | "derived_summary"
  | "legacy_unknown";

export type MemoryEvidenceOrigin = {
  kind: MemoryEvidenceOriginKind;
  account_id?: string | null;
  workspace_id?: string | null;
  runtime_id?: string | null;
  worker_id?: string | null;
  flow_selector?: string | null;
  flow_definition_id?: string | null;
  flow_definition_revision?: number | null;
};

export type MemorySourceRef = { segment_id: string; range: [number, number] };

export type MemoryStagingEvidence = {
  id: string;
  kind: string;
  entry_range: [number, number] | null;
  origin?: MemoryEvidenceOrigin | null;
  excerpt: string | null;
  summary: string | null;
};

export type MemorySourceEvidenceRef = {
  session_id: string | null;
  segment_id: string | null;
  entry_range: [number, number] | null;
  evidence_id: string | null;
  origin?: MemoryEvidenceOrigin | null;
  evidence_kind: string | null;
  label: string | null;
  summary: string | null;
};

export type MemoryStagingRecord = {
  schema_version: number;
  id: string;
  extract_run_id: string;
  source: MemorySourceRef;
  kind: MemoryCandidateKind;
  claim: string;
  why_useful: string;
  staleness: string | null;
  evidence: Array<MemoryStagingEvidence>;
  source_refs: Array<MemorySourceEvidenceRef>;
};

export type MemoryStagingEntry = {
  id: string;
  byte_len: number;
  record: MemoryStagingRecord;
};

export type MemoryStagingListResponse = {
  limit: number;
  returned_count: number;
  total_valid_count: number;
  invalid_count: number;
  truncated: boolean;
  order: string;
  record_authority: string;
  items: Array<MemoryStagingEntry>;
  diagnostics: Array<Diagnostic>;
};
