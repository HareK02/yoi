import type { PageLoad } from "./$types";
import { loadJson } from "#lib/workspace/api/http.ts";
import { mergeRequestDetailPath } from "#lib/workspace/api/merge-requests.ts";
import {
  parseMergeRequestDetailResponse,
  TICKET_BROWSER_API_LOAD_POLICY,
} from "#lib/workspace/api/ticket-browser.ts";

import { deferPage } from "#lib/workspace/navigation/deferred.ts";

export const load = deferPage(async ({ params, fetch, url }: Parameters<PageLoad>[0]) => {
  const query = new URLSearchParams({ limit: "100" });
  const after = url.searchParams.get("after");
  if (after !== null) query.set("after", after);
  const result = await loadJson(
    fetch,
    `${
      mergeRequestDetailPath(params.workspaceId, params.mergeRequestId)
    }?${query}`,
    undefined,
    parseMergeRequestDetailResponse,
    TICKET_BROWSER_API_LOAD_POLICY,
  );
  return {
    workspaceId: params.workspaceId,
    mergeRequestId: params.mergeRequestId,
    after,
    mergeRequest: result.data,
    error: result.error,
  };
});
