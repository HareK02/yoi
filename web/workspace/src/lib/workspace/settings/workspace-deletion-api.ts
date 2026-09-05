import type {
  WorkspaceDeletionOperationResponse,
  WorkspaceDeletionPreflightResponse,
  WorkspaceDeletionRequest,
} from "$lib/generated/workspace-api";
import { loadJson } from "$lib/workspace/api/http";
import {
  parseWorkspaceDeletionOperationResponse,
  parseWorkspaceDeletionPreflightResponse,
} from "$lib/workspace/api/workspace-model";

const deletionResponsePolicy = {
  maxResponseBytes: 2 * 1024 * 1024,
  diagnosticLabel: "Workspace deletion",
} as const;

async function deletionJson<T>(
  path: string,
  init: RequestInit | undefined,
  parse: (value: unknown) => T,
): Promise<T> {
  const result = await loadJson(
    fetch,
    path,
    init,
    parse,
    deletionResponsePolicy,
  );
  if (result.error !== null || result.data === null) {
    throw new Error(
      result.error ?? "Workspace deletion response is unavailable",
    );
  }
  return result.data;
}

export async function preflightWorkspaceDeletion(
  workspaceId: string,
): Promise<WorkspaceDeletionPreflightResponse> {
  return await deletionJson(
    `/api/workspaces/${encodeURIComponent(workspaceId)}/deletion`,
    undefined,
    parseWorkspaceDeletionPreflightResponse,
  );
}

export async function startWorkspaceDeletion(
  workspaceId: string,
  request: WorkspaceDeletionRequest,
): Promise<WorkspaceDeletionOperationResponse> {
  return await deletionJson(
    `/api/workspaces/${encodeURIComponent(workspaceId)}/deletion`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(request),
    },
    parseWorkspaceDeletionOperationResponse,
  );
}

export async function getWorkspaceDeletion(
  operationId: string,
): Promise<WorkspaceDeletionOperationResponse> {
  return await deletionJson(
    `/api/workspace-deletions/${encodeURIComponent(operationId)}`,
    undefined,
    parseWorkspaceDeletionOperationResponse,
  );
}
