import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import type { PageLoad } from "./$types";

export type RepositorySource = {
  kind: "local_path" | "remote_git";
  uri: string;
};

export type RepositorySummary = {
  repository_id: string;
  display_name: string;
  kind: string;
  provider: string;
  source: RepositorySource;
  default_selector?: string | null;
  observed: {
    status: string;
    observed_at?: string | null;
  };
  diagnostics: Array<{ code: string; message: string }>;
};

type RepositoryListResponse = {
  items: RepositorySummary[];
  diagnostics: Array<{ code: string; message: string }>;
};

export const load: PageLoad = async ({ fetch, params }) => {
  const repositories = await loadJson<RepositoryListResponse>(
    fetch,
    workspaceApiPath(params.workspaceId, "/repositories"),
  );
  return {
    workspaceId: params.workspaceId,
    repositories: repositories.data,
    repositoriesError: repositories.error,
  };
};
