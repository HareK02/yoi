import { loadJson, workspaceApiPath } from "$lib/workspace-api/http";
import type { ObjectiveListResponse } from "$lib/workspace-sidebar/types";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const apiPath = (path: string) => workspaceApiPath(params.workspaceId, path);
  const objectives = await loadJson<ObjectiveListResponse>(
    fetch,
    apiPath("/objectives"),
  );

  return {
    workspaceId: params.workspaceId,
    objectives: objectives.data,
    objectivesError: objectives.error,
  };
};
