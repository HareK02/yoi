export type WorkspaceResponse = {
  workspace_id: string;
  display_name: string;
  record_authority: string;
  extension_points: {
    event_stream: { status: string; note: string };
    runner_connection: { status: string; note: string };
  };
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

export type WorkerSummary = {
  id: string;
  label: string;
  status: string;
  detail?: string | null;
};
