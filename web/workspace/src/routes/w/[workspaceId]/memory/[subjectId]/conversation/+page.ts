import { error } from "@sveltejs/kit";
import { browserSession } from "#lib/workspace/auth/session-state.ts";
import { loadJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
import { MEMORY_API_LOAD_POLICY, parseSubjektivSubjectResponse } from "#lib/workspace/memory/api.ts";
import { deferPage } from "#lib/workspace/navigation/deferred.ts";
import type { PageLoad } from "./$types";

export const load = deferPage(async ({ fetch, params }: Parameters<PageLoad>[0]) => {
  const session = await browserSession.ensure();
  if (session.kind !== "ready" || session.value.session.kind !== "authenticated" || !session.value.actor) {
    error(503, "The conversation account could not be confirmed. Retry this page.");
  }
  const subject = await loadJson(
    fetch,
    workspaceApiPath(params.workspaceId, `/subjektiv/subjects/${encodeURIComponent(params.subjectId)}`),
    undefined,
    (value) => parseSubjektivSubjectResponse(value, params.subjectId),
    MEMORY_API_LOAD_POLICY,
  );
  return {
    workspaceId: params.workspaceId,
    subjectId: params.subjectId,
    accountId: session.value.actor.account_id,
    subject,
  };
});
