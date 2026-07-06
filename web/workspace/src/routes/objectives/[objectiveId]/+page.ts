import { loadJson } from "$lib/workspace-api/http";
import type {
  ObjectiveDetail,
  ObjectiveListResponse,
} from "$lib/workspace-sidebar/types";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const objectiveId = params.objectiveId;
  const [objectives, objective] = await Promise.all([
    loadJson<ObjectiveListResponse>(fetch, "/api/objectives"),
    loadJson<ObjectiveDetail>(
      fetch,
      `/api/objectives/${encodeURIComponent(objectiveId)}`,
    ),
  ]);

  return {
    objectiveId,
    objectives: objectives.data,
    objectivesError: objectives.error,
    objective: objective.data,
    objectiveError: objective.error,
  };
};
