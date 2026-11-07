import { loadJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
import {
  loadWorkspaceRepositoryDetail,
  loadWorkspaceRepositoryLog,
} from "#lib/workspace/api/repositories.ts";
import { parseWorkspaceRuntimeList } from "#lib/workspace/api/runtime-management.ts";
import type { PageLoad } from "./$types";
import { deferPage } from "#lib/workspace/navigation/deferred.ts";

export const load = deferPage(
  async ({ fetch, params }: Parameters<PageLoad>[0]) => {
    const workspaceId = params.workspaceId;
    const repositoryKey = params.repositoryKey;
    const [repositoryResult, logResult, runtimesResult] = await Promise.all([
      loadWorkspaceRepositoryDetail(fetch, workspaceId, repositoryKey),
      loadWorkspaceRepositoryLog(fetch, workspaceId, repositoryKey),
      loadJson<unknown>(
        fetch,
        workspaceApiPath(workspaceId, "/runtimes"),
      ),
    ]);

    const repository = repositoryResult.data;
    const repositoryError = repositoryResult.error;

    const log = logResult.data;
    const logError = logResult.error;

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
  },
);
