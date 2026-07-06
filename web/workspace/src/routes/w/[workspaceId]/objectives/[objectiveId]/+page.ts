import { loadJson, workspaceApiPath } from "$lib/workspace-api/http";
import type {
  ObjectiveDetail,
  ObjectiveListResponse,
} from "$lib/workspace-sidebar/types";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const apiPath = (path: string) => workspaceApiPath(params.workspaceId, path);
  const objectiveId = params.objectiveId;
  const [objectives, objective] = await Promise.all([
    loadJson<ObjectiveListResponse>(fetch, apiPath('/objectives')),
    loadJson<ObjectiveDetail>(
      fetch,
      apiPath(`/objectives/${encodeURIComponent(objectiveId)}`),
    ),
  ]);

  return {
    workspaceId: params.workspaceId,
    objectiveId,
    objectives: objectives.data,
    objectivesError: objectives.error,
    objective: objective.data,
    objectiveError: objective.error,
  };
};
