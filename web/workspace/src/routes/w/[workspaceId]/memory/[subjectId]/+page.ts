import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import {
  MEMORY_API_LOAD_POLICY,
  parseSubjektivMemoryQueryResponse,
  parseSubjektivResidentSurfaceResponse,
  parseSubjektivSubjectResponse,
} from "$lib/workspace/memory/api";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const subjectId = params.subjectId;
  const subjectPath = `/subjektiv/subjects/${encodeURIComponent(subjectId)}`;
  const [subject, surface, memories] = await Promise.all([
    loadJson(
      fetch,
      workspaceApiPath(params.workspaceId, subjectPath),
      undefined,
      (value) => parseSubjektivSubjectResponse(value, subjectId),
      MEMORY_API_LOAD_POLICY,
    ),
    loadJson(
      fetch,
      workspaceApiPath(params.workspaceId, `${subjectPath}/surface`),
      undefined,
      (value) => parseSubjektivResidentSurfaceResponse(value, subjectId),
      MEMORY_API_LOAD_POLICY,
    ),
    loadJson(
      fetch,
      `${workspaceApiPath(params.workspaceId, `${subjectPath}/memories`)}?limit=100`,
      undefined,
      parseSubjektivMemoryQueryResponse,
      MEMORY_API_LOAD_POLICY,
    ),
  ]);

  return {
    workspaceId: params.workspaceId,
    subjectId,
    subject,
    surface,
    memories,
  };
};
