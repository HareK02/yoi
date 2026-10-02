import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import {
  MEMORY_API_LOAD_POLICY,
  parseSubjektivMemoryListRevisionsResponse,
  parseSubjektivMemoryReadResponse,
  parseSubjektivSubjectResponse,
} from "$lib/workspace/memory/api";
import type { PageLoad } from "./$types";

const SAFE_INTEGER_TEXT = /^(0|[1-9][0-9]{0,15})$/;
const MAX_CURSOR_BYTES = 16_384;

function boundedCursor(
  value: string | null,
  maxBytes = MAX_CURSOR_BYTES,
): string | null {
  if (value === null || value.length === 0) return null;
  return new TextEncoder().encode(value).byteLength <= maxBytes ? value : null;
}

function safeIntegerQuery(
  source: URLSearchParams,
  key: string,
  positive = false,
): string | null {
  const value = source.get(key);
  if (value === null || !SAFE_INTEGER_TEXT.test(value)) return null;
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || (positive ? parsed < 1 : parsed < 0)) {
    return null;
  }
  return String(parsed);
}

export const load: PageLoad = async ({ fetch, params, url }) => {
  const subjectId = params.subjectId;
  const memoryId = params.memoryId;
  const subjectPath = `/subjektiv/subjects/${encodeURIComponent(subjectId)}`;
  const memoryPath = `${subjectPath}/memories/${encodeURIComponent(memoryId)}`;
  const detailQuery = new URLSearchParams({ limit: "1000" });
  for (
    const [key, positive] of [
      ["revision", true],
      ["offset", false],
      ["byte_offset", false],
    ] as const
  ) {
    const value = safeIntegerQuery(url.searchParams, key, positive);
    if (value !== null) detailQuery.set(key, value);
  }
  const evidenceCursor = boundedCursor(
    url.searchParams.get("evidence_cursor"),
    512,
  );
  if (evidenceCursor) detailQuery.set("evidence_cursor", evidenceCursor);
  const revisionCursor = boundedCursor(url.searchParams.get("revision_cursor"));
  const revisionQuery = new URLSearchParams({ limit: "100" });
  if (revisionCursor) revisionQuery.set("cursor", revisionCursor);

  const [subject, memory, revisions] = await Promise.all([
    loadJson(
      fetch,
      workspaceApiPath(params.workspaceId, subjectPath),
      undefined,
      (value) => parseSubjektivSubjectResponse(value, subjectId),
      MEMORY_API_LOAD_POLICY,
    ),
    loadJson(
      fetch,
      `${workspaceApiPath(params.workspaceId, memoryPath)}?${detailQuery}`,
      undefined,
      (value) => parseSubjektivMemoryReadResponse(value, memoryId),
      MEMORY_API_LOAD_POLICY,
    ),
    loadJson(
      fetch,
      `${
        workspaceApiPath(params.workspaceId, `${memoryPath}/revisions`)
      }?${revisionQuery}`,
      undefined,
      (value) => parseSubjektivMemoryListRevisionsResponse(value, memoryId),
      MEMORY_API_LOAD_POLICY,
    ),
  ]);

  return {
    workspaceId: params.workspaceId,
    subjectId,
    memoryId,
    revisionCursor,
    subject,
    memory,
    revisions,
  };
};
