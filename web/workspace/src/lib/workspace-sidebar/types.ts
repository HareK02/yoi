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

export type ObjectiveSummary = {
  id: string;
  title: string;
  state: string;
  updated_at?: string | null;
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
