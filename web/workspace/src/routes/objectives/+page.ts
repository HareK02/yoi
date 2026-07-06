import { loadJson } from "$lib/workspace-api/http";
import type { ObjectiveListResponse } from "$lib/workspace-sidebar/types";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch }) => {
  const objectives = await loadJson<ObjectiveListResponse>(
    fetch,
    "/api/objectives",
  );

  return {
    objectives: objectives.data,
    objectivesError: objectives.error,
  };
};
