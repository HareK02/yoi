export type ExtensionPoint = {
  status: string;
  note: string;
};

export type WorkspaceResponse = {
  workspace_id: string;
  display_name: string;
  record_authority: string;
  extension_points: {
    event_stream: ExtensionPoint;
    host_worker_bridge: ExtensionPoint;
  };
};

export type Diagnostic = {
  code: string;
  severity: string;
  message: string;
};

export type Host = {
  host_id: string;
  label: string;
  kind: string;
  status: string;
  observed_at: string;
  last_seen_at: string;
  capabilities: {
    local_pod_inspection: string;
    workspace_root: string;
    os: string;
    arch: string;
    max_workers: number;
  };
  diagnostics: Diagnostic[];
};

export type Worker = {
  worker_id: string;
  host_id: string;
  label: string;
  pod_name: string;
  role?: string;
  profile?: string;
  workspace_root?: string;
  state: string;
  status: string;
  last_seen_at?: string;
  implementation: { kind: string; pod_name: string };
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
  workspace_root: string;
  record_authority: string;
  git: GitRepositorySummary;
};

export type GitRepositorySummary = {
  status: string;
  root?: string | null;
  branch?: string | null;
  head?: string | null;
  dirty?: boolean | null;
  dirty_scope: string;
  remote?: GitRemoteSummary | null;
  diagnostics: Diagnostic[];
};

export type GitRemoteSummary = {
  name: string;
  url: string;
  redacted: boolean;
};

export type GitCommitSummary = {
  hash: string;
  subject: string;
  author_name: string;
  author_email: string;
  timestamp: string;
};

export type RepositoryDetailResponse = {
  workspace_id: string;
  item: RepositorySummary;
  source: string;
};

export type RepositoryLogResponse = {
  workspace_id: string;
  repository_id: string;
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
