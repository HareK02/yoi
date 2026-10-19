import { browserUrlSuffix } from "#lib/workspace/auth/routes.ts";
import { redirect } from "@sveltejs/kit";
import { loadJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
import {
  parseObjectiveDetail,
  TICKET_BROWSER_API_LOAD_POLICY,
} from "#lib/workspace/api/ticket-browser.ts";
import {
  canonicalResourceReference,
  resourceKey,
} from "#lib/workspace/resource-links.ts";
import type { PageLoad } from "./$types";
import { deferPage } from "#lib/workspace/navigation/deferred.ts";

export const load = deferPage(
  async ({ fetch, params, url }: Parameters<PageLoad>[0]) => {
    const apiPath = (path: string) =>
      workspaceApiPath(params.workspaceId, path);
    const objectiveId = resourceKey(params.objectiveId);
    const objective = await loadJson(
      fetch,
      apiPath(`/objectives/${encodeURIComponent(objectiveId)}`),
      undefined,
      parseObjectiveDetail,
      TICKET_BROWSER_API_LOAD_POLICY,
    );

    if (objective.data) {
      const canonical = canonicalResourceReference(
        objective.data.resource_key,
        objective.data.title,
      );
      if (params.objectiveId !== canonical) {
        redirect(
          308,
          `/w/${encodeURIComponent(params.workspaceId)}/objectives/${
            encodeURIComponent(canonical)
          }${browserUrlSuffix(url)}`,
        );
      }
    }

    return {
      workspaceId: params.workspaceId,
      objectiveId,
      objective: objective.data,
      objectiveError: objective.error,
    };
  },
);
