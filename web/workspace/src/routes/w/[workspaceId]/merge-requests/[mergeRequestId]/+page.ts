import type { PageLoad } from "./$types";
import { loadJson } from "$lib/workspace/api/http";
import { mergeRequestDetailPath } from "$lib/workspace/api/merge-requests";
import {
  parseMergeRequestDetailResponse,
  TICKET_BROWSER_API_LOAD_POLICY,
} from "$lib/workspace/api/ticket-browser";

export const load: PageLoad = async ({ params, fetch }) => {
  const result = await loadJson(
    fetch,
    `${
      mergeRequestDetailPath(params.workspaceId, params.mergeRequestId)
    }?limit=100`,
    undefined,
    parseMergeRequestDetailResponse,
    TICKET_BROWSER_API_LOAD_POLICY,
  );
  return {
    workspaceId: params.workspaceId,
    mergeRequestId: params.mergeRequestId,
    mergeRequest: result.data,
    error: result.error,
  };
};
