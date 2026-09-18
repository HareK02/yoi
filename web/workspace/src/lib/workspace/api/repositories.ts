import type {
  CreateWorkspaceRepositoryRequest,
  CreateWorkspaceRepositoryResponse,
  RepositoryDetailResponse,
  RepositoryListResponse,
} from "$lib/generated/repository-api.ts";
import {
  type ApiResult,
  loadJson,
  readBoundedJson,
  workspaceApiPath,
} from "$lib/workspace/api/http.ts";
import {
  parseCreateWorkspaceRepositoryResponse,
  parseRepositoryApiError,
  parseRepositoryDetailResponse,
  parseRepositoryListResponse,
} from "$lib/workspace/api/workspace-model.ts";

export const REPOSITORY_API_MAX_RESPONSE_BYTES = 2 * 1024 * 1024;

const REPOSITORY_API_LOAD_POLICY = {
  diagnosticLabel: "Repository API",
  maxResponseBytes: REPOSITORY_API_MAX_RESPONSE_BYTES,
} as const;

export function workspaceRepositoryListPath(workspaceId: string): string {
  return workspaceApiPath(workspaceId, "/repositories");
}

export function workspaceRepositoryDetailPath(
  workspaceId: string,
  repositoryKey: string,
): string {
  return workspaceApiPath(
    workspaceId,
    `/repositories/${encodeURIComponent(repositoryKey)}`,
  );
}

export async function loadWorkspaceRepositoryList(
  fetchFn: typeof fetch,
  workspaceId: string,
): Promise<ApiResult<RepositoryListResponse>> {
  return loadJson(
    fetchFn,
    workspaceRepositoryListPath(workspaceId),
    undefined,
    parseRepositoryListResponse,
    REPOSITORY_API_LOAD_POLICY,
  );
}

export async function loadWorkspaceRepositoryDetail(
  fetchFn: typeof fetch,
  workspaceId: string,
  repositoryKey: string,
): Promise<ApiResult<RepositoryDetailResponse>> {
  return loadJson(
    fetchFn,
    workspaceRepositoryDetailPath(workspaceId, repositoryKey),
    undefined,
    parseRepositoryDetailResponse,
    REPOSITORY_API_LOAD_POLICY,
  );
}

export async function createWorkspaceRepository(
  fetchFn: typeof fetch,
  workspaceId: string,
  request: CreateWorkspaceRepositoryRequest,
): Promise<ApiResult<CreateWorkspaceRepositoryResponse>> {
  const path = workspaceRepositoryListPath(workspaceId);
  try {
    const response = await fetchFn(path, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(request),
    });
    const payload = await readBoundedJson(
      response,
      REPOSITORY_API_MAX_RESPONSE_BYTES,
    );
    if (!response.ok) {
      try {
        const error = parseRepositoryApiError(payload);
        return { data: null, error: error.message };
      } catch {
        return {
          data: null,
          error: `Repository API request failed with HTTP ${response.status}`,
        };
      }
    }
    return {
      data: parseCreateWorkspaceRepositoryResponse(payload),
      error: null,
    };
  } catch {
    return { data: null, error: "Repository API response is invalid" };
  }
}
