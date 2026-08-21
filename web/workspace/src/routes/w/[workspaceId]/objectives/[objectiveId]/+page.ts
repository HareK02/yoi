import { redirect } from "@sveltejs/kit";
import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import {
  canonicalResourceReference,
  resourceKey,
} from "$lib/workspace/resource-links";
import type {
  ObjectiveDetail,
  ObjectiveListResponse,
} from "$lib/workspace/sidebar/types";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const apiPath = (path: string) => workspaceApiPath(params.workspaceId, path);
  const objectiveId = resourceKey(params.objectiveId);
  const [objectives, objective] = await Promise.all([
    loadJson<ObjectiveListResponse>(fetch, apiPath("/objectives")),
    loadJson<ObjectiveDetail>(
      fetch,
      apiPath(`/objectives/${encodeURIComponent(objectiveId)}`),
    ),
  ]);

  if (objective.data) {
    const canonical = canonicalResourceReference(
      objective.data.resource_key,
      objective.data.title,
    );
    if (params.objectiveId !== canonical) {
      redirect(
        308,
        `/w/${encodeURIComponent(params.workspaceId)}/objectives/${encodeURIComponent(canonical)}`,
      );
    }
  }

  return {
    workspaceId: params.workspaceId,
    objectiveId,
    objectives: objectives.data,
    objectivesError: objectives.error,
    objective: objective.data,
    objectiveError: objective.error,
  };
};
