import { loadJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
import {
  MEMORY_API_LOAD_POLICY,
  parseSubjektivMemoryListChangesResponse,
  parseSubjektivMemoryReadResponse,
  parseSubjektivSubjectResponse,
} from "#lib/workspace/memory/api.ts";
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
      ["offset", false],
      ["byte_offset", false],
    ] as const
  ) {
    const value = safeIntegerQuery(url.searchParams, key, positive);
    if (value !== null) detailQuery.set(key, value);
  }
  const changeId = boundedCursor(url.searchParams.get("change_id"), 512);
  if (changeId) detailQuery.set("change_id", changeId);
  const evidenceCursor = boundedCursor(
    url.searchParams.get("evidence_cursor"),
    512,
  );
  if (evidenceCursor) detailQuery.set("evidence_cursor", evidenceCursor);
  const changeCursor = boundedCursor(url.searchParams.get("change_cursor"));
  const changeQuery = new URLSearchParams({ limit: "100" });
  if (changeCursor) changeQuery.set("cursor", changeCursor);

  const [subject, memory, changes] = await Promise.all([
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
        workspaceApiPath(params.workspaceId, `${memoryPath}/changes`)
      }?${changeQuery}`,
      undefined,
      (value) => parseSubjektivMemoryListChangesResponse(value, memoryId),
      MEMORY_API_LOAD_POLICY,
    ),
  ]);

  return {
    workspaceId: params.workspaceId,
    subjectId,
    memoryId,
    changeCursor,
    subject,
    memory,
    changes,
  };
};
