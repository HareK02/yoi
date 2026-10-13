import { loadJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
import {
  parseObjectiveListResponse,
  TICKET_BROWSER_API_LOAD_POLICY,
} from "#lib/workspace/api/ticket-browser.ts";
import type { PageLoad } from "./$types";
import { deferPage } from "#lib/workspace/navigation/deferred.ts";

export const load = deferPage(async ({ fetch, params }: Parameters<PageLoad>[0]) => {
  const apiPath = (path: string) => workspaceApiPath(params.workspaceId, path);
  const objectives = await loadJson(
    fetch,
    apiPath("/objectives"),
    undefined,
    parseObjectiveListResponse,
    TICKET_BROWSER_API_LOAD_POLICY,
  );

  return {
    workspaceId: params.workspaceId,
    objectives: objectives.data,
    objectivesError: objectives.error,
  };
});
