import { loadJson, workspaceApiPath } from "$lib/workspace-api/http";
import type { Host, ListResponse } from "$lib/workspace-sidebar/types";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const apiPath = (path: string) => workspaceApiPath(params.workspaceId, path);
  const hosts = await loadJson<ListResponse<Host>>(fetch, apiPath("/hosts"));

  return {
    workspaceId: params.workspaceId,
    hosts: hosts.data,
    hostsError: hosts.error,
  };
};
