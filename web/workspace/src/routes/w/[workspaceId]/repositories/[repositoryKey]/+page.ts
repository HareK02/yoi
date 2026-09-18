import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import { loadWorkspaceRepositoryDetail } from "$lib/workspace/api/repositories";
import { parseWorkspaceRuntimeList } from "$lib/workspace/api/runtime-management";
import { parseRepositoryLogResponse } from "$lib/workspace/api/workspace-model";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const workspaceId = params.workspaceId;
  const repositoryKey = params.repositoryKey;
  const [repositoryResult, logResult, runtimesResult] = await Promise.all([
    loadWorkspaceRepositoryDetail(fetch, workspaceId, repositoryKey),
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

  const repository = repositoryResult.data;
  const repositoryError = repositoryResult.error;

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
