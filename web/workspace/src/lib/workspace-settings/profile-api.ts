import {
  workspaceApiJson,
  workspaceApiJsonWithBody,
} from "../workspace-api/http";
import type {
  ProfileSettingsMutationResponse,
  ProfileSettingsResponse,
  WorkspaceMetadataMutationResponse,
  WorkspaceMetadataSettingsResponse,
  WorkspaceProfileSourceDetailResponse,
  WorkspaceProfileSourceTreeFileResponse,
  WorkspaceProfileSourceTreeResponse,
} from "./profile-types";

export function fetchWorkspaceMetadataSettings(
  workspaceId: string,
): Promise<WorkspaceMetadataSettingsResponse> {
  return workspaceApiJson(
    `/api/w/${encodeURIComponent(workspaceId)}/settings/workspace`,
  );
}

export function updateWorkspaceMetadataSettings(
  workspaceId: string,
  request: { display_name: string; revision: string },
): Promise<WorkspaceMetadataMutationResponse> {
  return workspaceApiJsonWithBody(
    `/api/w/${encodeURIComponent(workspaceId)}/settings/workspace`,
    {
      method: "PUT",
      body: JSON.stringify(request),
    },
  );
}

export function fetchProfileSettings(
  workspaceId: string,
): Promise<ProfileSettingsResponse> {
  return workspaceApiJson(
    `/api/w/${encodeURIComponent(workspaceId)}/settings/profiles`,
  );
}

export function createProfileSource(
  workspaceId: string,
  request: {
    name: string;
    description?: string;
    content: string;
    registry_revision: string;
  },
): Promise<ProfileSettingsMutationResponse> {
  return workspaceApiJsonWithBody(
    `/api/w/${encodeURIComponent(workspaceId)}/settings/profiles`,
    {
      method: "POST",
      body: JSON.stringify(request),
    },
  );
}

export function updateProfileRegistry(
  workspaceId: string,
  request: {
    registry_revision: string;
    default_profile?: string | null;
    profiles: Array<
      {
        name: string;
        description?: string | null;
        profile_source_id?: string | null;
      }
    >;
  },
): Promise<ProfileSettingsMutationResponse> {
  return workspaceApiJsonWithBody(
    `/api/w/${encodeURIComponent(workspaceId)}/settings/profiles/registry`,
    {
      method: "PUT",
      body: JSON.stringify(request),
    },
  );
}

export function fetchProfileSource(
  workspaceId: string,
  sourceId: string,
): Promise<WorkspaceProfileSourceDetailResponse> {
  return workspaceApiJson(
    `/api/w/${encodeURIComponent(workspaceId)}/settings/profiles/${
      encodeURIComponent(sourceId)
    }`,
  );
}

export function updateProfileSource(
  workspaceId: string,
  sourceId: string,
  request: { content: string; revision: string },
): Promise<ProfileSettingsMutationResponse> {
  return workspaceApiJsonWithBody(
    `/api/w/${encodeURIComponent(workspaceId)}/settings/profiles/${
      encodeURIComponent(sourceId)
    }`,
    { method: "PUT", body: JSON.stringify(request) },
  );
}

export function deleteProfileSource(
  workspaceId: string,
  sourceId: string,
  request: { registry_revision: string; source_revision: string },
): Promise<ProfileSettingsMutationResponse> {
  return workspaceApiJsonWithBody(
    `/api/w/${encodeURIComponent(workspaceId)}/settings/profiles/${
      encodeURIComponent(sourceId)
    }`,
    { method: "DELETE", body: JSON.stringify(request) },
  );
}

export function fetchProfileSourceTree(
  workspaceId: string,
  sourceTreeId: string,
): Promise<WorkspaceProfileSourceTreeResponse> {
  return workspaceApiJson(
    `/api/w/${encodeURIComponent(workspaceId)}/settings/profiles/trees/${encodeURIComponent(sourceTreeId)}`,
  );
}

export function fetchProfileTreeFile(
  workspaceId: string,
  sourceTreeId: string,
  path: string,
): Promise<WorkspaceProfileSourceTreeFileResponse> {
  return workspaceApiJson(
    `/api/w/${encodeURIComponent(workspaceId)}/settings/profiles/trees/${encodeURIComponent(sourceTreeId)}/file?path=${encodeURIComponent(path)}`,
  );
}

export function writeProfileTreeFile(
  workspaceId: string,
  sourceTreeId: string,
  request: { path: string; content: string; revision?: string | null },
): Promise<WorkspaceProfileSourceTreeFileResponse> {
  return workspaceApiJsonWithBody(
    `/api/w/${encodeURIComponent(workspaceId)}/settings/profiles/trees/${encodeURIComponent(sourceTreeId)}/file`,
    { method: "PUT", body: JSON.stringify(request) },
  );
}

export function deleteProfileTreeFile(
  workspaceId: string,
  sourceTreeId: string,
  request: { path: string; revision: string },
): Promise<WorkspaceProfileSourceTreeResponse> {
  return workspaceApiJsonWithBody(
    `/api/w/${encodeURIComponent(workspaceId)}/settings/profiles/trees/${encodeURIComponent(sourceTreeId)}/file`,
    { method: "DELETE", body: JSON.stringify(request) },
  );
}
