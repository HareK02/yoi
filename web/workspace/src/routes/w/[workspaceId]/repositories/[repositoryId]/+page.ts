import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import type {
  RepositoryDetailResponse,
  RepositoryLogResponse,
} from "$lib/workspace/sidebar/types";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const apiPath = (path: string) => workspaceApiPath(params.workspaceId, path);
  const repositoryId = params.repositoryId;
  const [repository, log] = await Promise.all([
    loadJson<RepositoryDetailResponse>(
      fetch,
      apiPath(`/repositories/${encodeURIComponent(repositoryId)}`),
    ),
    loadJson<RepositoryLogResponse>(
      fetch,
      apiPath(`/repositories/${encodeURIComponent(repositoryId)}/log`),
    ),
  ]);

  return {
    repositoryId,
    repository: repository.data,
    repositoryError: repository.error,
    repositoryLog: log.data,
    repositoryLogError: log.error,
  };
};
