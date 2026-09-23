import type {
  MergeRequestDetailResponse,
  MergeRequestListResponse,
  MergeRequestThreadEvent,
} from "$lib/generated/ticket-api";
import { workspaceApiPath } from "./http";

export type MergeRequestDetail = MergeRequestDetailResponse;
export type MergeRequestListPage = MergeRequestListResponse;
export type { MergeRequestThreadEvent };

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
