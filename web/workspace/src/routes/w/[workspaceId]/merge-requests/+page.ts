import type { PageLoad } from "./$types";
import { loadJson } from "$lib/workspace/api/http";
import { mergeRequestCollectionPath } from "$lib/workspace/api/merge-requests";
import {
  parseMergeRequestListResponse,
  TICKET_BROWSER_API_LOAD_POLICY,
} from "$lib/workspace/api/ticket-browser";

export const load: PageLoad = async ({ params, fetch }) => {
  const result = await loadJson(
    fetch,
    `${mergeRequestCollectionPath(params.workspaceId)}?limit=100`,
    undefined,
    parseMergeRequestListResponse,
    TICKET_BROWSER_API_LOAD_POLICY,
  );
  return {
    workspaceId: params.workspaceId,
    mergeRequests: result.data,
    error: result.error,
  };
};
