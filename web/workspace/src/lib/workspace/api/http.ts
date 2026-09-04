import type {
  SkillCatalogResponse,
  SkillDetailResponse,
} from "$lib/generated/skill-api.ts";
import {
  parseSkillCatalogResponse,
  parseSkillDetailResponse,
} from "$lib/workspace/skills/api.ts";

export type ApiResult<T> = {
  data: T | null;
  error: string | null;
};

export type { SkillCatalogResponse, SkillDetailResponse };

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
  );
}

export async function loadJson<T>(
  fetchFn: typeof fetch,
  path: string,
  init?: RequestInit,
  parse: (value: unknown) => T = (value) => value as T,
): Promise<ApiResult<T>> {
  try {
    const response = await fetchFn(path, init);
    if (!response.ok) {
      const text = await response.text();
      return {
        data: null,
        error: text || `${path} request failed (${response.status})`,
      };
    }
    const payload: unknown = await response.json();
    return { data: parse(payload), error: null };
  } catch (error) {
    return {
      data: null,
      error: error instanceof Error ? error.message : `${path} request failed`,
    };
  }
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
