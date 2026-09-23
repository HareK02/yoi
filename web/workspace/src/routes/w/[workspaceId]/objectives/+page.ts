import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import {
  parseObjectiveListResponse,
  TICKET_BROWSER_API_LOAD_POLICY,
} from "$lib/workspace/api/ticket-browser";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
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
};
