import type {
  MergeRequestDetailResponse,
  MergeRequestListResponse,
  MergeRequestThreadEvent,
} from "#lib/generated/ticket-api.ts";
import { workspaceApiPath } from "./http";

export type MergeRequestDetail = MergeRequestDetailResponse;
export type MergeRequestListPage = MergeRequestListResponse;
export type { MergeRequestThreadEvent };

// 条件の評価・target正規化はBackendが所有する。URLとAPI間では値を保持するだけにする。
const queryKeys = [
  "repository_key",
  "selector_to",
  "phase",
  "state",
  "ticket_ref",
  "selector_from",
  "assigned_to_self",
  "limit",
  "cursor",
] as const;

export function mergeRequestQuery(search: URLSearchParams): URLSearchParams {
  const query = new URLSearchParams();
  for (const key of queryKeys) {
    const value = search.get(key);
    if (value) query.set(key, value);
  }
  if (!query.has("limit")) query.set("limit", "50");
  return query;
}

export function mergeRequestQueryPagePath(
  workspaceId: string,
  search: URLSearchParams,
  cursor: string | null,
): string {
  const query = mergeRequestQuery(search);
  if (cursor === null) query.delete("cursor");
  else query.set("cursor", cursor);
  return `${mergeRequestPagePath(workspaceId)}?${query}`;
}

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
