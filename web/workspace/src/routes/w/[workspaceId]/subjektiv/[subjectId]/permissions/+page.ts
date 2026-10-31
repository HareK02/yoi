import { loadJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
import {
  MEMORY_API_LOAD_POLICY,
  parseSubjektivSubjectResponse,
} from "#lib/workspace/memory/api.ts";
import { deferPage } from "#lib/workspace/navigation/deferred.ts";
import type { PageLoad } from "./$types";

export const load = deferPage(async (
  { fetch, params }: Parameters<PageLoad>[0],
) => ({
  workspaceId: params.workspaceId,
  subjectId: params.subjectId,
  subject: await loadJson(
    fetch,
    workspaceApiPath(
      params.workspaceId,
      `/subjektiv/subjects/${encodeURIComponent(params.subjectId)}`,
    ),
    undefined,
    (value) => parseSubjektivSubjectResponse(value, params.subjectId),
    MEMORY_API_LOAD_POLICY,
  ),
}));
