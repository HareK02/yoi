import { loadJson } from "$lib/workspace-api/http";
import type {
  RepositoryDetailResponse,
  RepositoryLogResponse,
  RepositoryTicketsResponse,
} from "$lib/workspace-sidebar/types";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const repositoryId = params.repositoryId;
  const [repository, log, tickets] = await Promise.all([
    loadJson<RepositoryDetailResponse>(
      fetch,
      `/api/repositories/${encodeURIComponent(repositoryId)}`,
    ),
    loadJson<RepositoryLogResponse>(
      fetch,
      `/api/repositories/${encodeURIComponent(repositoryId)}/log`,
    ),
    loadJson<RepositoryTicketsResponse>(
      fetch,
      `/api/repositories/${encodeURIComponent(repositoryId)}/tickets`,
    ),
  ]);

  return {
    repositoryId,
    repository: repository.data,
    repositoryError: repository.error,
    repositoryLog: log.data,
    repositoryLogError: log.error,
    repositoryTickets: tickets.data,
    repositoryTicketsError: tickets.error,
  };
};
