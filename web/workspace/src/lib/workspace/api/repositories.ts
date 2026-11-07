import type {
  CreateWorkspaceRepositoryRequest,
  CreateWorkspaceRepositoryResponse,
  RepositoryDetailResponse,
  RepositoryListResponse,
  RepositoryLogResponse,
} from "#lib/generated/repository-api.ts";
import type { DomainFailureDetails } from "#lib/generated/domain-failures.ts";
import {
  type ApiResult,
  readBoundedJson,
  ResponseByteLimitError,
  workspaceApiPath,
} from "#lib/workspace/api/http.ts";
import {
  parseCreateWorkspaceRepositoryResponse,
  parseRepositoryApiError,
  parseRepositoryDetailResponse,
  parseRepositoryListResponse,
  parseRepositoryLogResponse,
} from "#lib/workspace/api/workspace-model.ts";
import { decodeDomainFailureDetails } from "#lib/workspace/failure/domains.ts";
import { failureNotice } from "#lib/workspace/failure/model.ts";
import {
  type FailureReceipt,
  receiveProblem,
} from "#lib/workspace/failure/receive.ts";

export const REPOSITORY_API_MAX_RESPONSE_BYTES = 2 * 1024 * 1024;

export type RepositoryApiResult<T> = ApiResult<T> & {
  /** 公開契約の検証済みreceipt。HTTP statusや復旧判断を説明文から作らない。 */
  receipt?: FailureReceipt<DomainFailureDetails>;
};

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
): Promise<RepositoryApiResult<RepositoryListResponse>> {
  return repositoryJson(
    fetchFn,
    workspaceRepositoryListPath(workspaceId),
    undefined,
    parseRepositoryListResponse,
  );
}

export async function loadWorkspaceRepositoryDetail(
  fetchFn: typeof fetch,
  workspaceId: string,
  repositoryKey: string,
): Promise<RepositoryApiResult<RepositoryDetailResponse>> {
  return repositoryJson(
    fetchFn,
    workspaceRepositoryDetailPath(workspaceId, repositoryKey),
    undefined,
    parseRepositoryDetailResponse,
  );
}

export async function loadWorkspaceRepositoryLog(
  fetchFn: typeof fetch,
  workspaceId: string,
  repositoryKey: string,
): Promise<RepositoryApiResult<RepositoryLogResponse>> {
  return repositoryJson(
    fetchFn,
    `${workspaceRepositoryDetailPath(workspaceId, repositoryKey)}/log`,
    undefined,
    parseRepositoryLogResponse,
  );
}

export async function createWorkspaceRepository(
  fetchFn: typeof fetch,
  workspaceId: string,
  request: CreateWorkspaceRepositoryRequest,
): Promise<RepositoryApiResult<CreateWorkspaceRepositoryResponse>> {
  return repositoryJson(
    fetchFn,
    workspaceRepositoryListPath(workspaceId),
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(request),
    },
    parseCreateWorkspaceRepositoryResponse,
  );
}

async function repositoryJson<T>(
  fetchFn: typeof fetch,
  path: string,
  init: RequestInit | undefined,
  parse: (value: unknown) => T,
): Promise<RepositoryApiResult<T>> {
  let stage: "request" | "decode" | "validate" = "request";
  try {
    const response = await fetchFn(path, init);
    if (!response.ok) {
      return await repositoryResponseError(response, init?.method === "POST");
    }
    stage = "decode";
    const payload = await readBoundedJson(
      response,
      REPOSITORY_API_MAX_RESPONSE_BYTES,
    );
    stage = "validate";
    return { data: parse(payload), error: null };
  } catch (error) {
    return {
      data: null,
      error: error instanceof ResponseByteLimitError
        ? "Repository API response exceeds its byte limit"
        : stage === "request"
        ? "Repository API request could not be completed"
        : stage === "decode"
        ? "Repository API response could not be decoded as JSON"
        : "Repository API response is invalid",
    };
  }
}

async function repositoryResponseError(
  response: Response,
  allowLegacyCreate: boolean,
): Promise<RepositoryApiResult<never>> {
  const contentType = response.headers.get("content-type")?.split(";")[0]
    .trim().toLowerCase();
  // createの既存preflight/入力拒否はlegacy JSON。Problemの破損や未知versionを
  // legacyへ再解釈せず、list/detailへ不要な旧契約を広げない。
  if (allowLegacyCreate && contentType === "application/json") {
    try {
      const legacy = parseRepositoryApiError(
        await readBoundedJson(response, REPOSITORY_API_MAX_RESPONSE_BYTES),
      );
      return { data: null, error: legacy.message, status: response.status };
    } catch {
      return {
        data: null,
        error: `Repository API request failed with HTTP ${response.status}`,
        status: response.status,
      };
    }
  }
  const receipt = await receiveProblem(
    response,
    (envelope) =>
      envelope.domain === "repository"
        ? decodeDomainFailureDetails(envelope)
        : { kind: "unsupported" },
  );
  return {
    data: null,
    error: receipt.kind === "http"
      ? `Repository API request failed with HTTP ${response.status}`
      : failureNotice(receipt).explanation,
    status: response.status,
    receipt,
  };
}
