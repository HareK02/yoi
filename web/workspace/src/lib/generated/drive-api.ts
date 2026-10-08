// Generated from server-api. Do not edit by hand.
// Regenerate: cargo run -q -p server-api --features typescript --example generate_drive_api_types > web/workspace/src/lib/generated/drive-api.ts

export type DriveEntryRef = { workspace_id: string; node_id: string };

export type DriveEntryKind = "folder" | "file";

export type DriveEntry = {
  entry: DriveEntryRef;
  parent: DriveEntryRef | null;
  name: string;
  kind: DriveEntryKind;
  revision: string;
  /**
   * File length in bytes; absent for folders.
   */
  size: number | null;
  content_type: string | null;
  updated_by: string;
  /**
   * Timestamp projected from storage's updated_at_ms; not a creation time.
   */
  updated_at: string;
  /**
   * Stable relative authenticated URL in this same Workspace. No bearer token
   * or cross-Workspace link is embedded in this metadata.
   */
  latest_url: string;
};

export type DriveEntryQuery = { entry_workspace_id: string; id: string };

export type DriveListQuery = {
  entry_workspace_id: string;
  /**
   * Folder to list.
   */
  id: string;
  limit: number | null;
  /**
   * Opaque service-issued page cursor, not a bare node ID.
   */
  after: string | null;
};

export type DriveSearchQuery = {
  query: string;
  /**
   * Search file text as well as names only when explicitly requested.
   */
  include_text: boolean;
  limit: number | null;
  after: string | null;
};

export type DriveListResponse = {
  entries: Array<DriveEntry>;
  next_after: string | null;
};

export type DriveReadTextQuery = {
  entry_workspace_id: string;
  id: string;
  max_bytes: number;
};

export type DriveReadTextResponse = {
  entry: DriveEntry;
  text: string;
  truncated: boolean;
};

export type DriveReadChunkQuery = {
  entry_workspace_id: string;
  id: string;
  /**
   * Bind a multi-chunk read to the same file revision.
   */
  expected_revision: string;
  offset: number;
  length: number;
};

export type DriveDownloadQuery = {
  entry_workspace_id: string;
  id: string;
  expected_revision: string | null;
};

export type DriveMutation = {
  "operation": "create_folder";
  parent: DriveEntryRef;
  name: string;
} | {
  "operation": "create_text";
  parent: DriveEntryRef;
  name: string;
  text: string;
  content_type: string;
} | {
  "operation": "update_text";
  id: DriveEntryRef;
  expected_revision: string;
  text: string;
  content_type: string;
} | {
  "operation": "relocate";
  id: DriveEntryRef;
  expected_revision: string;
  parent: DriveEntryRef;
  name: string;
} | { "operation": "delete"; id: DriveEntryRef; expected_revision: string };

export type DriveMutationRequest = {
  request_id: string;
  mutation: DriveMutation;
};

export type DriveMutationResponse = {
  request_id: string;
  /**
   * A deleted entry has no live metadata.
   */
  entry: DriveEntry | null;
};

export type DriveUploadOperation = "create" | "update";

export type DriveUploadQuery = {
  operation: DriveUploadOperation;
  request_id: string;
  entry_workspace_id: string;
  parent_id: string | null;
  name: string | null;
  id: string | null;
  expected_revision: string | null;
  content_type: string;
  /**
   * Values above the file limit still decode as u32 so the service returns
   * typed Limit rather than a generic request-decoding error.
   */
  size: number;
  sha256: string;
};

export type DriveRequestState = "uncommitted" | "committed";

export type DriveRequestStatusResponse = {
  request_id: string;
  state: DriveRequestState;
  response: DriveMutationResponse | null;
};

export type DriveAccess = "read_only" | "read_write";

export type DriveGrantCreateRequest = {
  runtime_id: string;
  worker_id: string;
  access: DriveAccess;
};

export type DriveGrantResponse = {
  grant_id: string;
  workspace_id: string;
  runtime_id: string;
  worker_id: string;
  access: DriveAccess;
  revoked: boolean;
  created_by: string;
  created_at: string;
  revoked_by: string | null;
  revoked_at: string | null;
};

export type DriveGrantListQuery = {
  limit: number | null;
  /**
   * Canonical grant ID cursor within this Workspace.
   */
  after: string | null;
};

export type DriveGrantListResponse = {
  grants: Array<DriveGrantResponse>;
  next_after: string | null;
};

export type DriveApiErrorCode =
  | "denied"
  | "not_found"
  | "conflict"
  | "invalid"
  | "limit"
  | "storage_unavailable"
  | "outcome_unknown";

export type DriveFailureClassification = "not_committed" | "unknown";

export type DriveApiError = {
  code: DriveApiErrorCode;
  classification: DriveFailureClassification;
  message: string;
};
