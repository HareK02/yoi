import { redirect } from "@sveltejs/kit";
import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import {
  canonicalResourceReference,
  resourceKey,
} from "$lib/workspace/resource-links";
import type { Worker } from "$lib/workspace/sidebar/types";
import type { PageLoad } from "./$types";

export const load = (async ({ fetch, params }) => {
  const reference = resourceKey(params.workerRef);
  const result = await loadJson<Worker>(
    fetch,
    workspaceApiPath(
      params.workspaceId,
      `/workers/${encodeURIComponent(reference)}`,
    ),
  );
  if (result.data?.resource_key) {
    const canonical = canonicalResourceReference(
      result.data.resource_key,
      result.data.display_name,
    );
    if (params.workerRef !== canonical) {
      redirect(
        308,
        `/w/${encodeURIComponent(params.workspaceId)}/workers/${encodeURIComponent(canonical)}`,
      );
    }
  }
  return {
    workspaceId: params.workspaceId,
    worker: result.data,
    workerError: result.error,
  };
}) satisfies PageLoad;
