import { loadJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
import {
  MEMORY_API_LOAD_POLICY,
  parseSubjektivSubjectListResponse,
} from "#lib/workspace/memory/api.ts";
import type { PageLoad } from "./$types";

const MAX_CURSOR_BYTES = 16_384;

function boundedCursor(value: string | null): string | null {
  if (value === null || value.length === 0) return null;
  return new TextEncoder().encode(value).byteLength <= MAX_CURSOR_BYTES
    ? value
    : null;
}

export const load: PageLoad = async ({ fetch, params, url }) => {
  const cursor = boundedCursor(url.searchParams.get("cursor"));
  const query = new URLSearchParams({ limit: "100" });
  if (cursor) query.set("cursor", cursor);
  return {
    workspaceId: params.workspaceId,
    cursor,
    subjects: await loadJson(
      fetch,
      `${workspaceApiPath(params.workspaceId, "/subjektiv/subjects")}?${query}`,
      undefined,
      parseSubjektivSubjectListResponse,
      MEMORY_API_LOAD_POLICY,
    ),
  };
};
