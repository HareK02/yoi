import type {
  ProfileSettingsResponse,
  WorkspaceMetadataMutationResponse,
  WorkspaceMetadataSettingsResponse,
} from "./profile-types";

export type WorkspaceProfileApi = {
  getMetadata(workspaceId: string): Promise<WorkspaceMetadataSettingsResponse>;
  updateMetadata(
    workspaceId: string,
    displayName: string,
    expectedRevision: string,
  ): Promise<WorkspaceMetadataMutationResponse>;
  getProfiles(workspaceId: string): Promise<ProfileSettingsResponse>;
};

async function requestJson<T>(
  input: RequestInfo | URL,
  init?: RequestInit,
): Promise<T> {
  const response = await fetch(input, init);
  if (!response.ok) {
    throw new Error(`request failed: ${response.status}`);
  }
  return (await response.json()) as T;
}

export async function fetchWorkspaceMetadataSettings(
  workspaceId: string,
): Promise<WorkspaceMetadataSettingsResponse> {
  return await requestJson<WorkspaceMetadataSettingsResponse>(
    `/api/w/${encodeURIComponent(workspaceId)}/settings/workspace`,
  );
}

export async function updateWorkspaceMetadataSettings(
  workspaceId: string,
  request: { display_name: string; revision: string },
): Promise<WorkspaceMetadataMutationResponse> {
  return await requestJson<WorkspaceMetadataMutationResponse>(
    `/api/w/${encodeURIComponent(workspaceId)}/settings/workspace`,
    {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(request),
    },
  );
}

export async function fetchProfileSettings(
  workspaceId: string,
): Promise<ProfileSettingsResponse> {
  return await requestJson<ProfileSettingsResponse>(
    `/api/w/${encodeURIComponent(workspaceId)}/settings/profiles`,
  );
}

export function createWorkspaceProfileApi(): WorkspaceProfileApi {
  return {
    getMetadata: fetchWorkspaceMetadataSettings,
    async updateMetadata(workspaceId, displayName, expectedRevision) {
      return await updateWorkspaceMetadataSettings(workspaceId, {
        display_name: displayName,
        revision: expectedRevision,
      });
    },
    getProfiles: fetchProfileSettings,
  };
}
