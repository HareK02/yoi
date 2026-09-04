import { SKILL_API_LIMITS } from "$lib/generated/skill-api.ts";
import type {
  SkillCatalogResponse,
  SkillDetailResponse,
} from "$lib/generated/skill-api.ts";
import {
  parseSkillCatalogResponse,
  parseSkillDetailResponse,
  SkillApiContractError,
} from "$lib/workspace/skills/api.ts";

export type ApiResult<T> = {
  data: T | null;
  error: string | null;
};

export type { SkillCatalogResponse, SkillDetailResponse };

type JsonLoadPolicy = {
  diagnosticLabel: string;
  maxResponseBytes: number;
};

const SKILL_API_LOAD_POLICY: JsonLoadPolicy = {
  diagnosticLabel: "Skill API",
  maxResponseBytes: SKILL_API_LIMITS.maxResponseBytes,
};

class ResponseByteLimitError extends Error {}

function normalizePath(path: string): string {
  if (!path || path === "/") return "";
  return path.startsWith("/") ? path : `/${path}`;
}

export function workspaceRoute(workspaceId: string, path = ""): string {
  return `/w/${encodeURIComponent(workspaceId)}${normalizePath(path)}`;
}

export function workspaceApiPath(workspaceId: string, path = ""): string {
  return `/api/w/${encodeURIComponent(workspaceId)}${normalizePath(path)}`;
}

export function workspaceSkillCatalogPath(workspaceId: string): string {
  return workspaceApiPath(workspaceId, "/skills");
}

export function workspaceSkillDetailPath(
  workspaceId: string,
  name: string,
): string {
  return workspaceApiPath(workspaceId, `/skills/${encodeURIComponent(name)}`);
}

export function workspaceSkillActivationPath(
  workspaceId: string,
  name: string,
): string {
  return workspaceApiPath(
    workspaceId,
    `/skills/${encodeURIComponent(name)}/activate`,
  );
}

export async function loadWorkspaceSkillCatalog(
  fetchFn: typeof fetch,
  workspaceId: string,
): Promise<ApiResult<SkillCatalogResponse>> {
  return loadJson<SkillCatalogResponse>(
    fetchFn,
    workspaceSkillCatalogPath(workspaceId),
    undefined,
    parseSkillCatalogResponse,
    SKILL_API_LOAD_POLICY,
  );
}

export async function loadWorkspaceSkillDetail(
  fetchFn: typeof fetch,
  workspaceId: string,
  name: string,
): Promise<ApiResult<SkillDetailResponse>> {
  return loadJson<SkillDetailResponse>(
    fetchFn,
    workspaceSkillDetailPath(workspaceId, name),
    undefined,
    parseSkillDetailResponse,
    SKILL_API_LOAD_POLICY,
  );
}

export async function loadJson<T>(
  fetchFn: typeof fetch,
  path: string,
  init?: RequestInit,
  parse: (value: unknown) => T = (value) => value as T,
  policy?: JsonLoadPolicy,
): Promise<ApiResult<T>> {
  try {
    const response = await fetchFn(path, init);
    if (!response.ok) {
      if (policy) {
        await response.body?.cancel();
        return {
          data: null,
          error:
            `${policy.diagnosticLabel} request failed with HTTP ${response.status}`,
        };
      }
      const text = await response.text();
      return {
        data: null,
        error: text || `${path} request failed (${response.status})`,
      };
    }
    const payload: unknown = policy
      ? await readBoundedJson(response, policy.maxResponseBytes)
      : await response.json();
    return { data: parse(payload), error: null };
  } catch (error) {
    if (policy) {
      const diagnostic = error instanceof SkillApiContractError
        ? error.message
        : error instanceof ResponseByteLimitError
        ? `${policy.diagnosticLabel} response exceeds its byte limit`
        : `${policy.diagnosticLabel} response is invalid`;
      return { data: null, error: diagnostic.slice(0, 256) };
    }
    return {
      data: null,
      error: error instanceof Error ? error.message : `${path} request failed`,
    };
  }
}

async function readBoundedJson(
  response: Response,
  maxBytes: number,
): Promise<unknown> {
  const contentLength = response.headers.get("content-length");
  if (contentLength !== null) {
    const parsedLength = Number(contentLength);
    if (Number.isFinite(parsedLength) && parsedLength > maxBytes) {
      await response.body?.cancel();
      throw new ResponseByteLimitError();
    }
  }

  const reader = response.body?.getReader();
  if (!reader) {
    throw new Error("response body is unavailable");
  }
  const chunks: Uint8Array[] = [];
  let totalBytes = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      totalBytes += value.byteLength;
      if (totalBytes > maxBytes) {
        await reader.cancel();
        throw new ResponseByteLimitError();
      }
      chunks.push(value);
    }
  } finally {
    reader.releaseLock();
  }

  const bytes = new Uint8Array(totalBytes);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  return JSON.parse(text) as unknown;
}

async function requireJson<T>(response: Response, path: string): Promise<T> {
  if (!response.ok) {
    const text = await response.text();
    throw new Error(text || `${path} request failed (${response.status})`);
  }
  return (await response.json()) as T;
}

export async function workspaceApiJson<T>(path: string): Promise<T> {
  return requireJson<T>(await fetch(path), path);
}

export async function workspaceApiJsonWithBody<T>(
  path: string,
  init: RequestInit,
): Promise<T> {
  return requireJson<T>(
    await fetch(path, {
      headers: {
        "content-type": "application/json",
        ...(init.headers ?? {}),
      },
      ...init,
    }),
    path,
  );
}

export function formatDate(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return date.toLocaleString();
}
