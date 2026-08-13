import type {
  ConfigCommitRequest,
  ConfigEntry,
  EvaluatedConfigCandidate,
  WorkspaceConfigTreeResponse,
} from "./types.ts";

function sourceTreeUrl(workspaceId: string): string {
  return `/api/w/${encodeURIComponent(workspaceId)}/config/source-tree`;
}

async function readJson<T>(response: Response): Promise<T> {
  if (!response.ok) {
    const body = await response.text();
    throw new Error(body || `${response.status} ${response.statusText}`);
  }
  return await response.json() as T;
}

export async function fetchConfigTree(
  workspaceId: string,
  fetcher: typeof fetch = fetch,
): Promise<WorkspaceConfigTreeResponse> {
  return await readJson(
    await fetcher(sourceTreeUrl(workspaceId), {
      headers: { accept: "application/json" },
    }),
  );
}

export async function fetchConfigEntry(
  workspaceId: string,
  path: string,
  fetcher: typeof fetch = fetch,
): Promise<ConfigEntry> {
  return await readJson(
    await fetcher(
      `${sourceTreeUrl(workspaceId)}/entries/${encodeURIComponent(path)}`,
      { headers: { accept: "application/json" } },
    ),
  );
}

export async function previewConfigTree(
  workspaceId: string,
  request: Omit<ConfigCommitRequest, "base_revision" | "base_digest">,
  fetcher: typeof fetch = fetch,
): Promise<EvaluatedConfigCandidate> {
  return await readJson(
    await fetcher(`${sourceTreeUrl(workspaceId)}/preview`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(request),
    }),
  );
}

export async function commitConfigTree(
  workspaceId: string,
  request: ConfigCommitRequest,
  fetcher: typeof fetch = fetch,
): Promise<WorkspaceConfigTreeResponse> {
  return await readJson(
    await fetcher(`${sourceTreeUrl(workspaceId)}/commit`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(request),
    }),
  );
}
