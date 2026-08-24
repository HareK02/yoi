export type WorkspaceCatalogRecord = {
  workspace_id: string;
  owner_account_id: string | null;
  display_name: string;
  state: string;
  created_at: string;
  updated_at: string;
};

export type RepositorySourceKind =
  | "local_path"
  | "file"
  | "ssh"
  | "http"
  | "https"
  | "invalid";

export type WorkspaceRepositoryRecord = {
  workspace_id: string;
  repository_id: string;
  name: string;
  kind: string;
  provider: string | null;
  source: {
    kind: RepositorySourceKind;
    uri: string;
  };
  default_ref: string | null;
  source_revision: number;
  source_fingerprint: string;
  observed_status: "unverified" | "ready" | "invalid";
  observed_at: string | null;
};

export type WorkspaceCatalogItem = WorkspaceCatalogRecord & {
  repositories: WorkspaceRepositoryRecord[];
  repository_error?: string;
};

export type CreateWorkspaceRequest = {
  operation_key: string;
  display_name: string;
  repository: {
    uri: string;
    display_name: string | null;
    default_ref: string | null;
  };
};

export type CreateWorkspaceResponse = {
  workspace: WorkspaceCatalogRecord;
  repository: WorkspaceRepositoryRecord;
  config_revision: number;
  request_fingerprint: string;
  replayed: boolean;
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
  return await fetchJson<WorkspaceCatalogRecord[]>(
    fetcher,
    "/api/workspaces?limit=200",
  );
}

export async function listWorkspaceRepositories(
  fetcher: Fetch,
  workspaceId: string,
): Promise<WorkspaceRepositoryRecord[]> {
  return await fetchJson<WorkspaceRepositoryRecord[]>(
    fetcher,
    `/api/w/${encodeURIComponent(workspaceId)}/repositories`,
  );
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
  return await fetchJson<CreateWorkspaceResponse>(fetcher, "/api/workspaces", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(request),
  });
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

async function fetchJson<T>(
  fetcher: Fetch,
  input: string,
  init?: RequestInit,
): Promise<T> {
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
  return await response.json() as T;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
