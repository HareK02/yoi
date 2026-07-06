import { loadJson, workspaceApiPath } from "$lib/workspace-api/http";
import type { Host, ListResponse, Worker } from "$lib/workspace-sidebar/types";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const apiPath = (path: string) => workspaceApiPath(params.workspaceId, path);
  const [hosts, workers] = await Promise.all([
    loadJson<ListResponse<Host>>(fetch, apiPath("/hosts")),
    loadJson<ListResponse<Worker>>(fetch, apiPath("/workers")),
  ]);

  return {
    workspaceId: params.workspaceId,
    hosts: hosts.data,
    hostsError: hosts.error,
    workers: workers.data,
    workersError: workers.error,
  };
};
