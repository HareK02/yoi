import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import type { Worker } from "$lib/workspace/sidebar/types";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const worker = await loadJson<Worker>(
    fetch,
    workspaceApiPath(
      params.workspaceId,
      `/runtimes/${encodeURIComponent(params.runtimeId)}/workers/${
        encodeURIComponent(params.workerId)
      }`,
    ),
  );

  return {
    workspaceId: params.workspaceId,
    runtimeId: params.runtimeId,
    workerId: params.workerId,
    worker: worker.data,
    workerError: worker.error,
  };
};
