import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import {
  MEMORY_API_LOAD_POLICY,
  parseSubjektivMemoryQueryResponse,
  parseSubjektivResidentSurfaceResponse,
  parseSubjektivSubjectResponse,
} from "$lib/workspace/memory/api";
import type { PageLoad } from "./$types";

const MAX_CURSOR_BYTES = 16_384;

function boundedCursor(value: string | null): string | null {
  if (value === null || value.length === 0) return null;
  return new TextEncoder().encode(value).byteLength <= MAX_CURSOR_BYTES
    ? value
    : null;
}

export const load: PageLoad = async ({ fetch, params, url }) => {
  const subjectId = params.subjectId;
  const subjectPath = `/subjektiv/subjects/${encodeURIComponent(subjectId)}`;
  const cursor = boundedCursor(url.searchParams.get("cursor"));
  const memoryQuery = new URLSearchParams({ limit: "100" });
  if (cursor) memoryQuery.set("cursor", cursor);
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
      `${
        workspaceApiPath(params.workspaceId, `${subjectPath}/memories`)
      }?${memoryQuery}`,
      undefined,
      parseSubjektivMemoryQueryResponse,
      MEMORY_API_LOAD_POLICY,
    ),
  ]);

  return {
    workspaceId: params.workspaceId,
    subjectId,
    cursor,
    subject,
    surface,
    memories,
  };
};
