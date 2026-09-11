import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import { parseWorkspaceRuntimeList } from "$lib/workspace/api/runtime-management";
import {
  parseRepositoryDetailResponse,
  parseRepositoryLogResponse,
} from "$lib/workspace/api/workspace-model";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const workspaceId = params.workspaceId;
  const repositoryKey = params.repositoryKey;
  const [repositoryResult, logResult, runtimesResult] = await Promise.all([
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
    loadJson<unknown>(
      fetch,
      workspaceApiPath(workspaceId, "/runtimes"),
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

  let runtimes = null;
  let runtimesError = runtimesResult.error;
  if (runtimesResult.data !== null) {
    try {
      runtimes = parseWorkspaceRuntimeList(runtimesResult.data);
    } catch (cause) {
      runtimesError = cause instanceof Error
        ? cause.message
        : "invalid Runtime summary response";
    }
  }

  return {
    repositoryKey,
    repository,
    repositoryError,
    repositoryLog: log,
    repositoryLogError: logError,
    runtimes,
    runtimesError,
  };
};
