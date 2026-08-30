import type {
  Event as PodProtocolEvent,
  Method as PodProtocolMethod,
  Segment as PodProtocolSegment,
} from "$lib/generated/protocol";

export type { PodProtocolEvent, PodProtocolMethod, PodProtocolSegment };

export type ExtensionPoint = {
  status: string;
  note: string;
  diagnostics: Diagnostic[];
};

export type WorkspaceResponse = {
  workspace_id: string;
  display_name: string;
  record_authority: string;
  extension_points: {
    event_stream: ExtensionPoint;
    host_worker_bridge: ExtensionPoint;
    companion_console: ExtensionPoint;
  };
};

export type Diagnostic = {
  code: string;
  severity: string;
  message: string;
};

export type Runtime = {
  runtime_id: string;
  label: string;
  kind: string;
  status: string;
  host_ids: string[];
  worker_creation_available: boolean;
  os: string;
  arch: string;
  diagnostics: Diagnostic[];
  management?: {
    built_in: boolean;
    config_managed: boolean;
    removable: boolean;
    endpoint_configured: boolean;
    token_ref_configured: boolean;
  };
};

export type Host = {
  runtime_id: string;
  host_id: string;
  label: string;
  kind: string;
  status: string;
  observed_at: string;
  last_seen_at: string | null;
  os: string;
  arch: string;
  diagnostics: Diagnostic[];
};

export type WorkerCapabilities = {
  can_stop: boolean;
  can_spawn_followup: boolean;
};

export type Worker = {
  runtime_id: string;
  worker_id: string;
  resource_key: string;
  host_id: string;
  display_name: string;
  label: string;
  profile?: string | null;
  singleton_key?: string | null;
  tags: string[];
  workspace: { visibility: string; identity: string };
  state: string;
  pinned?: boolean;
  retention_state?: string;
  last_seen_at?: string | null;
  implementation: { kind: string; display_hint: string };
  capabilities: WorkerCapabilities;
  working_directory?: WorkingDirectorySummary | null;
  diagnostics: Diagnostic[];
};

export type WorkerOperationState = "accepted" | "unsupported" | "rejected";

export type WorkerLaunchRuntimeOption = {
  runtime_id: string;
  display_name: string;
  built_in: boolean;
  worker_creation_available: boolean;
  working_directory_required: boolean;
  status: string;
  diagnostics: Diagnostic[];
};

export type WorkerLaunchProfileCandidate = {
  id: string;
  label: string;
  description: string;
};

export type WorkingDirectoryRepositoryOption = {
  id: string;
  display_name: string;
  default_selector?: string | null;
};

export type WorkingDirectoryOccupancy = {
  runtime_id: string;
  worker_id: string;
  display_name: string;
  linked_at: string;
};

export type WorkingDirectorySummary = {
  working_directory_id: string;
  repository_id: string;
  creation_selector?: string | null;
  creation_ref?: string | null;
  current_selector?: string | null;
  current_ref?: string | null;
  materializer_kind: string;
  status: string;
  cleanliness?: string | null;
  primary_worker_id?: string | null;
  occupied_by?: WorkingDirectoryOccupancy | null;
  cleanup_target: {
    kind: string;
    working_directory_id: string;
    repository_id: string;
  };
};

export type BrowserWorkingDirectoryCreateResponse = {
  workspace_id: string;
  item: WorkingDirectorySummary;
  diagnostics: Diagnostic[];
};

export type BrowserWorkingDirectoryListResponse = {
  workspace_id: string;
  items: WorkingDirectorySummary[];
  diagnostics: Diagnostic[];
};

export type CleanupTargetKind =
  | "worker_delete"
  | "workdir_clean_cleanup"
  | "workdir_dirty_discard"
  | "workdir_record_delete";

export type CleanupWorkerCandidate = {
  target_id: string;
  action: CleanupTargetKind;
  worker_id: string;
  runtime_worker_id: string;
  runtime_id: string;
  reason: string;
  blocking_reason?: string | null;
  pinned: boolean;
  retention_state: string;
  linked_workdir_ids: string[];
  running_linked: boolean;
  estimated_reclaim_bytes?: number | null;
};

export type CleanupWorkdirCandidate = {
  target_id: string;
  action: CleanupTargetKind;
  workdir_id: string;
  runtime_id: string;
  repository_id: string;
  reason: string;
  blocking_reason?: string | null;
  linked_worker_ids: string[];
  linked_running_worker_ids: string[];
  running_linked: boolean;
  pinned_linked: boolean;
  file_status: string;
  cleanliness: string;
  estimated_reclaim_bytes?: number | null;
};

export type RuntimeCleanupPlanResponse = {
  workspace_id: string;
  runtime_id: string;
  generated_at: string;
  revision: string;
  digest: string;
  workers: CleanupWorkerCandidate[];
  workdirs: CleanupWorkdirCandidate[];
  diagnostics: Diagnostic[];
};

export type RuntimeCleanupExecutionResponse = {
  workspace_id: string;
  runtime_id: string;
  executed_at: string;
  results: {
    target_id: string;
    action: CleanupTargetKind;
    status: string;
    message: string;
  }[];
  plan_after: RuntimeCleanupPlanResponse;
  diagnostics: Diagnostic[];
};

export type BrowserWorkerWorkingDirectorySelection = {
  working_directory_id: string;
  relative_cwd?: string | null;
};

export type BrowserWorkingDirectoryCreateRequest = {
  runtime_id: string;
  repository_id: string;
  selector?: string | null;
};

export type WorkerLaunchOptionsResponse = {
  workspace_id: string;
  runtimes: WorkerLaunchRuntimeOption[];
  default_profile?: string | null;
  profiles: WorkerLaunchProfileCandidate[];
  repositories: WorkingDirectoryRepositoryOption[];
  working_directories: WorkingDirectorySummary[];
  diagnostics: Diagnostic[];
};

export type BrowserCreateWorkerResponse = {
  workspace_id: string;
  runtime_id: string;
  worker_id: string;
  console_href: string;
  worker: Worker;
  diagnostics: Diagnostic[];
};

export type WorkerInputResult = {
  state: WorkerOperationState;
  runtime_id: string;
  worker_id: string;
  diagnostics: Diagnostic[];
};

export type ListResponse<T> = {
  workspace_id: string;
  limit: number;
  items: T[];
  source: string;
  diagnostics: Diagnostic[];
};

export type RepositorySummary = {
  id: string;
  display_name: string;
  kind: string;
  provider: string;
  source: {
    kind: "local_path" | "file" | "ssh" | "http" | "https" | "invalid";
    uri: string;
  };
  source_revision: number;
  source_fingerprint: string;
  observed_status: "unverified" | "ready" | "invalid";
  observed_at?: string | null;
  default_selector?: string | null;
  record_authority: string;
  git?: GitRepositorySummary | null;
  diagnostics?: Diagnostic[];
};

export type GitRepositorySummary = {
  status: string;
  branch?: string | null;
  head?: string | null;
  dirty: boolean;
  remotes: GitRemoteSummary[];
};

export type GitRemoteSummary = {
  name: string;
  fetch_url: string;
};

export type GitCommitSummary = {
  hash: string;
  short_hash: string;
  summary: string;
  author_name: string;
  author_email: string;
  author_date: string;
  parents: string[];
  refs: string[];
};

export type RepositoryListResponse = {
  workspace_id: string;
  items: RepositorySummary[];
  source: string;
  diagnostics: Diagnostic[];
};

export type RepositoryDetailResponse = {
  workspace_id: string;
  item: RepositorySummary;
  source: string;
};

export type RepositoryLogResponse = {
  workspace_id: string;
  repository_id: string;
  default_selector?: string | null;
  limit: number;
  items: GitCommitSummary[];
  diagnostics: Diagnostic[];
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

export type MemorySourceRef = {
  segment_id: string;
  range: [number, number];
};

export type MemoryStagingEvidence = {
  id: string;
  kind: string;
  entry_range?: [number, number] | null;
  excerpt?: string | null;
  summary?: string | null;
};

export type MemorySourceEvidenceRef = {
  session_id?: string | null;
  segment_id?: string | null;
  entry_range?: [number, number] | null;
  evidence_id?: string | null;
  evidence_kind?: string | null;
  label?: string | null;
  summary?: string | null;
};

export type MemoryStagingRecord = {
  schema_version: number;
  id: string;
  extract_run_id: string;
  source: MemorySourceRef;
  kind: MemoryCandidateKind;
  claim: string;
  why_useful: string;
  staleness?: string | null;
  evidence?: MemoryStagingEvidence[];
  source_refs?: MemorySourceEvidenceRef[];
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
  items: MemoryStagingEntry[];
};

export type {
  DerivedTicketRelation,
  TicketDetail,
  TicketEventDetail,
  TicketListResponse,
  TicketRelation,
  TicketRelationBlocker,
  TicketRelationNotice,
  TicketRelationView,
  TicketSummary,
} from "$lib/generated/ticket-api";

export type ObjectiveSummary = {
  id: string;
  resource_key: string;
  title: string;
  state: string;
  updated_at?: string | null;
  summary: string;
  linked_tickets?: string[];
  record_source?: string;
};

export type ObjectiveLinkedTicketSummary = {
  id: string;
  resource_key: string;
  title: string;
  state: string;
};

export type ObjectiveDetail = {
  id: string;
  resource_key: string;
  title: string;
  state: string;
  created_at?: string | null;
  updated_at?: string | null;
  linked_tickets: string[];
  linked_ticket_summaries: ObjectiveLinkedTicketSummary[];
  body: string;
  body_truncated: boolean;
  record_source: string;
};

export type InvalidProjectRecord = {
  label: string;
  reason: string;
};

export type ObjectiveListResponse = {
  workspace_id: string;
  limit: number;
  items: ObjectiveSummary[];
  invalid_records: InvalidProjectRecord[];
  record_authority: string;
};

export type CompanionState =
  | "ready"
  | "busy"
  | "error"
  | "timeout"
  | "cancelled"
  | "accepted"
  | "rejected";

export type CompanionTransportSummary = {
  kind: string;
  completion: string;
  limitation: string;
};

export type CompanionStatusResponse = {
  state: CompanionState;
  worker?: Worker | null;
  transport: CompanionTransportSummary;
  diagnostics: Diagnostic[];
};

export type CompanionTranscriptItem = {
  sequence: number;
  role: "user" | "assistant" | "system" | string;
  content: string;
  created_at: string;
  source: string;
  status: string;
};

export type CompanionTranscriptProjection = {
  state: CompanionState;
  start: number;
  limit: number;
  total_items: number;
  next_start?: number | null;
  items: CompanionTranscriptItem[];
  diagnostics: Diagnostic[];
};

export type CompanionMessageRequest = {
  content: string;
};

export type CompanionMessageResponse = {
  state: CompanionState;
  worker?: Worker | null;
  user_item?: CompanionTranscriptItem | null;
  assistant_item?: CompanionTranscriptItem | null;
  transcript: CompanionTranscriptProjection;
  diagnostics: Diagnostic[];
};
