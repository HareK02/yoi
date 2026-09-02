import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import {
  parseRepositoryDetailResponse,
  parseRepositoryLogResponse,
} from "$lib/workspace/api/workspace-model";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const workspaceId = params.workspaceId;
  const repositoryKey = params.repositoryKey;
  const [repositoryResult, logResult] = await Promise.all([
    loadJson<unknown>(
      fetch,
      workspaceApiPath(
        workspaceId,
        `/repositories/${encodeURIComponent(repositoryKey)}`,
      ),
    ),
    loadJson<unknown>(
      fetch,
      workspaceApiPath(
        workspaceId,
        `/repositories/${encodeURIComponent(repositoryKey)}/log`,
      ),
    ),
  ]);

  let repository = null;
  let repositoryError = repositoryResult.error;
  if (repositoryResult.data !== null) {
    try {
      repository = parseRepositoryDetailResponse(repositoryResult.data);
    } catch (cause) {
      repositoryError = cause instanceof Error
        ? cause.message
        : "invalid repository detail response";
    }
  }

  let log = null;
  let logError = logResult.error;
  if (logResult.data !== null) {
    try {
      log = parseRepositoryLogResponse(logResult.data);
    } catch (cause) {
      logError = cause instanceof Error
        ? cause.message
        : "invalid repository log response";
    }
  }

  return {
    repositoryKey,
    repository,
    repositoryError,
    repositoryLog: log,
    repositoryLogError: logError,
  };
};
