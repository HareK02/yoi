// Generated from workspace-api. Do not edit by hand.
// Regenerate: cargo run -q -p workspace-api --features typescript --example generate_worker_launch_api_types > web/workspace/src/lib/generated/worker-launch-api.ts

import type { Segment } from "./protocol";

export type DiagnosticSeverity = "info" | "warning" | "error";

export type Diagnostic = {
  code: string;
  severity: DiagnosticSeverity;
  message: string;
};

export type WorkingDirectoryMaterializerKind =
  | "runtime_git_cache"
  | "local_git_worktree";

export type WorkingDirectoryStatusKind =
  | "active"
  | "cleanup_pending"
  | "corrupted"
  | "not_found"
  | "unknown";

export type WorkingDirectoryCleanupTarget = {
  kind: string;
  working_directory_id: string;
  repository_key: string;
};

export type RuntimeWorkingDirectoryCleanupTarget = {
  kind: string;
  working_directory_id: string;
  repository_id: string;
};

export type RuntimeWorkingDirectorySummary = {
  working_directory_id: string;
  repository_id: string;
  creation_selector?: string | null;
  creation_ref?: string | null;
  creation_tree?: string | null;
  current_selector?: string | null;
  current_ref?: string | null;
  current_tree?: string | null;
  observed_at_epoch_seconds?: number | null;
  materializer_kind: WorkingDirectoryMaterializerKind;
  cleanup_target?: RuntimeWorkingDirectoryCleanupTarget | null;
  status: WorkingDirectoryStatusKind;
  cleanliness?: string | null;
  primary_worker_id?: string | null;
  occupied_by?: WorkingDirectoryOccupancy | null;
};

export type WorkingDirectoryOccupancy = {
  runtime_id: string;
  worker_id: string;
  display_name: string;
  linked_at: string;
};

export type WorkingDirectorySummary = {
  working_directory_id: string;
  repository_key: string;
  creation_selector?: string | null;
  creation_ref?: string | null;
  creation_tree?: string | null;
  current_selector?: string | null;
  current_ref?: string | null;
  current_tree?: string | null;
  observed_at_epoch_seconds?: number | null;
  materializer_kind: WorkingDirectoryMaterializerKind;
  cleanup_target?: WorkingDirectoryCleanupTarget | null;
  status: WorkingDirectoryStatusKind;
  cleanliness?: string | null;
  primary_worker_id?: string | null;
  occupied_by?: WorkingDirectoryOccupancy | null;
};

export type WorkerWorkspaceSummary = {
  visibility: string;
  identity: string;
  workspace_id?: string | null;
};

export type WorkerImplementationSummary = {
  kind: string;
  display_hint: string;
};

export type WorkerCapabilitySummary = {
  can_stop: boolean;
  can_spawn_followup: boolean;
};

export type WorkerLaunchWorkerSummary = {
  runtime_id: string;
  worker_id: string;
  host_id: string;
  display_name: string;
  label: string;
  profile: string | null;
  singleton_key: string | null;
  tags: Array<string>;
  workspace: WorkerWorkspaceSummary;
  state: string;
  last_seen_at: string | null;
  pinned: boolean;
  retention_state: string;
  implementation: WorkerImplementationSummary;
  capabilities: WorkerCapabilitySummary;
  working_directory?: RuntimeWorkingDirectorySummary | null;
  diagnostics: Array<Diagnostic>;
};

export type WorkerLaunchRuntimeOption = {
  runtime_id: string;
  display_name: string;
  built_in: boolean;
  worker_creation_available: boolean;
  working_directory_required: boolean;
  status: string;
  diagnostics: Array<Diagnostic>;
};

export type WorkerLaunchProfileCandidate = {
  id: string;
  label: string;
  description: string;
};

export type WorkingDirectoryRepositoryOption = {
  repository_key: string;
  default_selector?: string | null;
};

export type WorkerLaunchOptionsResponse = {
  workspace_id: string;
  runtimes: Array<WorkerLaunchRuntimeOption>;
  default_profile: string | null;
  profiles: Array<WorkerLaunchProfileCandidate>;
  repositories: Array<WorkingDirectoryRepositoryOption>;
  working_directories: Array<WorkingDirectorySummary>;
  diagnostics: Array<Diagnostic>;
};

export type BrowserWorkerWorkingDirectorySelection = {
  working_directory_id: string;
  relative_cwd: string | null;
};

export type CreateWorkspaceWorkerTicketAssignmentRequest = {
  ticket_id: string;
  operation_id: string;
};

export type CreateWorkspaceWorkerRequest = {
  runtime_id: string;
  display_name: string;
  profile: string | null;
  ticket_assignment: CreateWorkspaceWorkerTicketAssignmentRequest | null;
  initial_submit: Array<Segment>;
  working_directory: BrowserWorkerWorkingDirectorySelection | null;
  /**
   * Backend idempotency key used only for authenticated Worker-owned spawn/control.
   */
  control_operation_id: string | null;
};

export type BrowserCreateWorkerResponse = {
  workspace_id: string;
  runtime_id: string;
  worker_id: string;
  console_href: string;
  worker: WorkerLaunchWorkerSummary;
  diagnostics: Array<Diagnostic>;
};

export type BrowserWorkspaceOrchestratorResponse = {
  workspace_id: string;
  online: boolean;
  disposition: string;
  worker?: WorkerLaunchWorkerSummary | null;
  diagnostics: Array<Diagnostic>;
};
