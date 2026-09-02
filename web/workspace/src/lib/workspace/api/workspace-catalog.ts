import {
  parseRepositoryListResponse,
  parseWorkspaceCatalogResponse,
  parseWorkspaceCreateResponse,
  type RepositorySummary,
  type WorkspaceCreateResponse,
  type WorkspaceSummary,
} from "$lib/workspace/api/workspace-model";

export type WorkspaceCatalogRecord = WorkspaceSummary;
export type WorkspaceCatalogItem = WorkspaceCatalogRecord & {
  repositories: RepositorySummary[];
  repository_error?: string;
};
export type CreateWorkspaceResponse = WorkspaceCreateResponse;

export type CreateWorkspaceRequest = {
  operation_key: string;
  display_name: string;
  repository: {
    repository_key: string;
    uri: string;
    default_ref: string | null;
  };
};

export class WorkspaceCatalogError extends Error {
  constructor(
    public readonly status: number | null,
    message: string,
  ) {
    super(message);
    this.name = "WorkspaceCatalogError";
  }
}

type Fetch = typeof globalThis.fetch;

export async function listWorkspaces(
  fetcher: Fetch,
): Promise<WorkspaceCatalogRecord[]> {
  return parseWorkspaceCatalogResponse(
    await fetchJson(fetcher, "/api/workspaces?limit=200"),
  );
}

export async function listWorkspaceRepositories(
  fetcher: Fetch,
  workspaceId: string,
): Promise<RepositorySummary[]> {
  return parseRepositoryListResponse(
    await fetchJson(
      fetcher,
      `/api/w/${encodeURIComponent(workspaceId)}/repositories`,
    ),
  ).items;
}

export async function loadWorkspaceCatalog(
  fetcher: Fetch,
): Promise<WorkspaceCatalogItem[]> {
  const workspaces = await listWorkspaces(fetcher);
  return await Promise.all(
    workspaces.map(async (workspace) => {
      try {
        return {
          ...workspace,
          repositories: await listWorkspaceRepositories(
            fetcher,
            workspace.workspace_id,
          ),
        };
      } catch (error) {
        return {
          ...workspace,
          repositories: [],
          repository_error: errorMessage(error),
        };
      }
    }),
  );
}

export async function createWorkspace(
  fetcher: Fetch,
  request: CreateWorkspaceRequest,
): Promise<CreateWorkspaceResponse> {
  return parseWorkspaceCreateResponse(
    await fetchJson(fetcher, "/api/workspaces", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(request),
    }),
  );
}

export function creationErrorMessage(error: unknown): string {
  if (!(error instanceof WorkspaceCatalogError)) {
    return `Network error. The same operation can be retried safely. ${
      errorMessage(error)
    }`;
  }
  switch (error.status) {
    case 400:
      return `Validation failed. ${error.message}`;
    case 401:
    case 403:
      return `You are not authorized to create this Workspace. ${error.message}`;
    case 409:
      return `Creation conflicts with current Backend state. ${error.message}`;
    default:
      return `Workspace creation failed. The same operation can be retried safely. ${error.message}`;
  }
}

export function createOperationKey(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return `web-workspace-create-${crypto.randomUUID()}`;
  }
  return `web-workspace-create-${Date.now()}-${
    Math.random().toString(16).slice(2)
  }`;
}

async function fetchJson(
  fetcher: Fetch,
  input: string,
  init?: RequestInit,
): Promise<unknown> {
  let response: Response;
  try {
    response = await fetcher(input, init);
  } catch (error) {
    throw new WorkspaceCatalogError(null, errorMessage(error));
  }
  if (!response.ok) {
    let detail = `${response.status} ${response.statusText}`.trim();
    try {
      const body = await response.json();
      if (typeof body?.message === "string") detail = body.message;
      else if (typeof body?.error === "string") detail = body.error;
    } catch {
      // Preserve the bounded status text when the Backend did not return JSON.
    }
    throw new WorkspaceCatalogError(response.status, detail);
  }
  return await response.json() as unknown;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
