import { redirect } from "@sveltejs/kit";
import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import { parseWorkerSummary } from "$lib/workspace/api/workers";
import {
  canonicalResourceReference,
  resourceKey,
} from "$lib/workspace/resource-links";
import type { PageLoad } from "./$types";

export const load = (async ({ fetch, params }) => {
  const reference = resourceKey(params.workerRef);
  const result = await loadJson(
    fetch,
    workspaceApiPath(
      params.workspaceId,
      `/workers/${encodeURIComponent(reference)}`,
    ),
    undefined,
    parseWorkerSummary,
    { diagnosticLabel: "Worker API", maxResponseBytes: 8 * 1024 * 1024 },
  );
  if (result.data?.resource_key) {
    const canonical = canonicalResourceReference(
      result.data.resource_key,
      result.data.display_name,
    );
    if (params.workerRef !== canonical) {
      redirect(
        308,
        `/w/${encodeURIComponent(params.workspaceId)}/workers/${encodeURIComponent(canonical)}/console`,
      );
    }
  }

  return {
    workspaceId: params.workspaceId,
    runtimeId: result.data?.runtime_id ?? "",
    workerId: result.data?.worker_id ?? "",
    worker: result.data,
    workerError: result.error,
  };
}) satisfies PageLoad;
