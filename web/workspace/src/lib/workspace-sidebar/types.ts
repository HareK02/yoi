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

export type RuntimeCapabilities = {
  can_list_hosts: boolean;
  can_list_workers: boolean;
  can_get_worker: boolean;
  can_spawn_worker: boolean;
  can_stop_worker: boolean;
  can_accept_input: boolean;
  has_workspace_fs: boolean;
  has_shell: boolean;
  has_git: boolean;
  supports_worktrees: boolean;
  supports_backend_internal_tools: boolean;
  workspace_scope: string;
  os: string;
  arch: string;
  max_workers: number;
};

export type Runtime = {
  runtime_id: string;
  label: string;
  kind: string;
  status: string;
  host_ids: string[];
  capabilities: RuntimeCapabilities;
  diagnostics: Diagnostic[];
};

export type Host = {
  runtime_id: string;
  host_id: string;
  label: string;
  kind: string;
  status: string;
  observed_at: string;
  last_seen_at: string | null;
  capabilities: RuntimeCapabilities;
  diagnostics: Diagnostic[];
};

export type WorkerCapabilities = {
  can_accept_input: boolean;
  can_stop: boolean;
  can_spawn_followup: boolean;
};

export type Worker = {
  runtime_id: string;
  worker_id: string;
  host_id: string;
  label: string;
  role?: string | null;
  profile?: string | null;
  workspace: { visibility: string; identity: string };
  state: string;
  status: string;
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
  can_spawn_worker: boolean;
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

export type WorkingDirectorySummary = {
  working_directory_id: string;
  repository_id: string;
  requested_selector?: string | null;
  materializer_kind: string;
  dirty_state_policy: string;
  resolved_commit: string;
  resolved_tree?: string | null;
  status: string;
  management_kind?: "backend_managed" | "runtime_unmanaged" | string | null;
  cleanup_policy: string;
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
  policy?: {
    dirty_state?: "clean_point_only";
    cleanup?: "manual_or_worker_stop";
  };
};

export type WorkerLaunchOptionsResponse = {
  workspace_id: string;
  runtimes: WorkerLaunchRuntimeOption[];
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
  event_id?: number | null;
  diagnostics: Diagnostic[];
};

export type ClientWorkerEventWsEnvelope = {
  event_id: string;
  runtime_id: string;
  worker_id: string;
  payload: PodProtocolEvent;
};

export type ClientWorkerEventWsDiagnostic = {
  code: string;
  message: string;
};

export type ClientWorkerEventWsFrame =
  | { kind: "event"; envelope: ClientWorkerEventWsEnvelope }
  | { kind: "diagnostic"; diagnostic: ClientWorkerEventWsDiagnostic };

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

export type TicketSummary = {
  id: string;
  title: string;
  state: string;
  priority?: string | null;
  updated_at?: string | null;
  queued_by?: string | null;
  queued_at?: string | null;
  record_source?: string;
};

export type TicketKanbanColumn = {
  state: string;
  items: TicketSummary[];
};

export type RepositoryTicketsResponse = {
  workspace_id: string;
  repository_id: string;
  limit: number;
  columns: TicketKanbanColumn[];
  invalid_records: InvalidProjectRecord[];
  record_authority: string;
  source: string;
  diagnostics: Diagnostic[];
};

export type ObjectiveSummary = {
  id: string;
  title: string;
  state: string;
  updated_at?: string | null;
  summary: string;
  linked_tickets?: string[];
  record_source?: string;
};

export type ObjectiveDetail = {
  id: string;
  title: string;
  state: string;
  created_at?: string | null;
  updated_at?: string | null;
  linked_tickets: string[];
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
