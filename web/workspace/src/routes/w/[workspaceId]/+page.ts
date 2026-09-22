import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import {
  parseHostListResponse,
  type HostListResponse,
} from "$lib/workspace/api/workspace-model";
import type { PageLoad } from "./$types";

const HOST_API_LOAD_POLICY = {
  diagnosticLabel: "Host API",
  maxResponseBytes: 4 * 1024 * 1024,
} as const;

export const load: PageLoad = async ({ fetch, params }) => {
  const apiPath = (path: string) => workspaceApiPath(params.workspaceId, path);
  const hosts = await loadJson<HostListResponse>(
    fetch,
    apiPath("/hosts"),
    undefined,
    parseHostListResponse,
    HOST_API_LOAD_POLICY,
  );

  return {
    workspaceId: params.workspaceId,
    hosts: hosts.data,
    hostsError: hosts.error,
  };
};
