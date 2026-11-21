import type { PageLoad } from "./$types";
import { loadJson } from "#lib/workspace/api/http.ts";
import { mergeRequestCollectionPath, mergeRequestQuery } from "#lib/workspace/api/merge-requests.ts";
import {
  parseMergeRequestListResponse,
  TICKET_BROWSER_API_LOAD_POLICY,
} from "#lib/workspace/api/ticket-browser.ts";

import { deferPage } from "#lib/workspace/navigation/deferred.ts";

export const load = deferPage(async ({ params, fetch, url }: Parameters<PageLoad>[0]) => {
  const query = mergeRequestQuery(url.searchParams);
  const result = await loadJson(
    fetch,
    `${mergeRequestCollectionPath(params.workspaceId)}?${query}`,
    undefined,
    parseMergeRequestListResponse,
    TICKET_BROWSER_API_LOAD_POLICY,
  );
  return {
    workspaceId: params.workspaceId,
    query: query.toString(),
    mergeRequests: result.data,
    error: result.error,
  };
});
