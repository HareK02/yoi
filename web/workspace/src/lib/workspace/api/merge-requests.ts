import type { MergeRequestListResponse } from "$lib/generated/ticket-api";
import { workspaceApiPath } from "./http";

export type MergeRequestState = "open" | "merged" | "closed";

export type MergeRequestActor = {
  runtime_id: string;
  worker_id: string;
  assignment_id: string;
};

export type MergeRequestThreadEvent = {
  kind: string;
  sequence: number;
  at: string;
  [key: string]: unknown;
};

export type MergeRequestRecord = {
  merge_request_id: string;
  workspace_id: string;
  repository_key: string;
  selector_from: string | null;
  selector_to: string;
  ticket_ids: string[];
  state: MergeRequestState;
  opened_by: MergeRequestActor;
  created_at: string;
  updated_at: string;
  thread: MergeRequestThreadEvent[];
};

export type MergeRequestRefObservation = {
  status: string;
  ref: string | null;
  observed_at: string;
};

export type MergeRequestDetail = MergeRequestRecord & {
  source: MergeRequestRefObservation;
  target: MergeRequestRefObservation;
  linked_tickets: Array<{ ticket_id: string; key: string | null }>;
};

export type MergeRequestListPage = MergeRequestListResponse;

export function mergeRequestCollectionPath(workspaceId: string): string {
  return workspaceApiPath(workspaceId, "/merge-requests");
}

export function mergeRequestDetailPath(
  workspaceId: string,
  mergeRequestId: string,
): string {
  return workspaceApiPath(
    workspaceId,
    `/merge-requests/${encodeURIComponent(mergeRequestId)}`,
  );
}

export function mergeRequestPagePath(
  workspaceId: string,
  mergeRequestId?: string,
): string {
  const root = `/w/${encodeURIComponent(workspaceId)}/merge-requests`;
  return mergeRequestId
    ? `${root}/${encodeURIComponent(mergeRequestId)}`
    : root;
}
