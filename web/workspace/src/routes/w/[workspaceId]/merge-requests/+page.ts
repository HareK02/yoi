import type { PageLoad } from "./$types";
import { loadJson } from "#lib/workspace/api/http.ts";
import { mergeRequestCollectionPath } from "#lib/workspace/api/merge-requests.ts";
import {
  parseMergeRequestListResponse,
  TICKET_BROWSER_API_LOAD_POLICY,
} from "#lib/workspace/api/ticket-browser.ts";

import { deferPage } from "#lib/workspace/navigation/deferred.ts";

export const load = deferPage(async ({ params, fetch }: Parameters<PageLoad>[0]) => {
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
});
