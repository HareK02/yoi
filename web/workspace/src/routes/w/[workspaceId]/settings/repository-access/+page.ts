import { workspaceApiPath } from "$lib/workspace/api/http";
import {
  parseRepositoryAccessProjection,
  parseRepositorySshCredentials,
  parseRepositorySshHostTrusts,
} from "$lib/workspace/api/repository-access";
import { loadRepositoryAccessJson } from "$lib/workspace/api/repository-access-loader";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const workspaceId = params.workspaceId;
  const accessProjection = await loadRepositoryAccessJson(
    fetch,
    workspaceApiPath(workspaceId, "/settings/repository-access"),
    parseRepositoryAccessProjection,
  );
  const [credentials, hostTrusts] = await Promise.all([
    loadRepositoryAccessJson(
      fetch,
      workspaceApiPath(workspaceId, "/settings/repository-access/credentials"),
      parseRepositorySshCredentials,
    ),
    loadRepositoryAccessJson(
      fetch,
      workspaceApiPath(workspaceId, "/settings/repository-access/host-trusts"),
      parseRepositorySshHostTrusts,
    ),
  ]);

  return {
    workspaceId,
    credentials,
    hostTrusts,
    accessProjection,
  };
};
