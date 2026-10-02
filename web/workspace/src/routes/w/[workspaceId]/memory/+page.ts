import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import {
  MEMORY_API_LOAD_POLICY,
  parseSubjektivSubjectListResponse,
} from "$lib/workspace/memory/api";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  return {
    workspaceId: params.workspaceId,
    subjects: await loadJson(
      fetch,
      `${workspaceApiPath(params.workspaceId, "/subjektiv/subjects")}?limit=100`,
      undefined,
      parseSubjektivSubjectListResponse,
      MEMORY_API_LOAD_POLICY,
    ),
  };
};
