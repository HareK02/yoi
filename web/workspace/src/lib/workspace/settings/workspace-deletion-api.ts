import type {
  WorkspaceDeletionOperationResponse,
  WorkspaceDeletionPreflightResponse,
  WorkspaceDeletionRequest,
} from "$lib/generated/workspace-api";
import {
  parseWorkspaceDeletionOperationResponse,
  parseWorkspaceDeletionPreflightResponse,
} from "$lib/workspace/api/workspace-model";

async function responseJson(
  response: Response,
  context: string,
): Promise<unknown> {
  const value: unknown = await response.json().catch(() => null);
  if (!response.ok) {
    const message = typeof value === "object" && value !== null &&
        "error" in value && typeof value.error === "string"
      ? value.error
      : `${context} failed (${response.status})`;
    throw new Error(message);
  }
  return value;
}

export async function preflightWorkspaceDeletion(
  workspaceId: string,
): Promise<WorkspaceDeletionPreflightResponse> {
  const response = await fetch(
    `/api/workspaces/${encodeURIComponent(workspaceId)}/deletion`,
  );
  return parseWorkspaceDeletionPreflightResponse(
    await responseJson(response, "Workspace deletion preflight"),
  );
}

export async function startWorkspaceDeletion(
  workspaceId: string,
  request: WorkspaceDeletionRequest,
): Promise<WorkspaceDeletionOperationResponse> {
  const response = await fetch(
    `/api/workspaces/${encodeURIComponent(workspaceId)}/deletion`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(request),
    },
  );
  return parseWorkspaceDeletionOperationResponse(
    await responseJson(response, "Workspace deletion"),
  );
}

export async function getWorkspaceDeletion(
  operationId: string,
): Promise<WorkspaceDeletionOperationResponse> {
  const response = await fetch(
    `/api/workspace-deletions/${encodeURIComponent(operationId)}`,
  );
  return parseWorkspaceDeletionOperationResponse(
    await responseJson(response, "Workspace deletion status"),
  );
}
