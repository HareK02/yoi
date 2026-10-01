import { workspaceApiPath } from "#lib/workspace/api/http.ts";
import {
  parseRepositoryAccessProjection,
  parseRepositorySshCredentials,
  parseRepositorySshHostTrusts,
  parseRepositorySshPublicKey,
} from "#lib/workspace/api/repository-access.ts";
import {
  loadRepositoryAccessPermissionGate,
  loadRepositoryAccessSection,
} from "#lib/workspace/api/repository-access-loader.ts";
import type { Load } from "@sveltejs/kit";

const WORKSPACE_DEFAULT_CREDENTIAL_ID = "workspace-default";

export const load = (async ({ fetch, params }) => {
  const workspaceId = params.workspaceId;
  const accessProjectionResult = await loadRepositoryAccessPermissionGate(
    fetch,
    workspaceApiPath(workspaceId, "/settings/repository-access"),
    parseRepositoryAccessProjection,
    "Repository bindings",
  );
  const [credentialsResult, hostTrustsResult] = await Promise.all([
    loadRepositoryAccessSection(
      fetch,
      workspaceApiPath(workspaceId, "/settings/repository-access/credentials"),
      parseRepositorySshCredentials,
      "SSH credentials",
    ),
    loadRepositoryAccessSection(
      fetch,
      workspaceApiPath(workspaceId, "/settings/repository-access/host-trusts"),
      parseRepositorySshHostTrusts,
      "Pinned SSH host keys",
    ),
  ]);

  const credentials = credentialsResult.data ?? [];
  const defaultCredential = credentials.find(
    (credential) =>
      credential.credential_id === WORKSPACE_DEFAULT_CREDENTIAL_ID,
  );
  const defaultPublicKeyResult = defaultCredential
    ? await loadRepositoryAccessSection(
      fetch,
      workspaceApiPath(
        workspaceId,
        `/settings/repository-access/credentials/${
          encodeURIComponent(defaultCredential.credential_id)
        }/public-key`,
      ),
      parseRepositorySshPublicKey,
      "The Workspace default public key",
    )
    : { data: null, error: null };

  return {
    workspaceId,
    credentials,
    credentialsError: credentialsResult.error,
    publicKeys: defaultPublicKeyResult.data
      ? [defaultPublicKeyResult.data]
      : [],
    defaultPublicKeyError: defaultPublicKeyResult.error,
    hostTrusts: hostTrustsResult.data ?? [],
    hostTrustsError: hostTrustsResult.error,
    accessProjection: accessProjectionResult.data,
    accessProjectionError: accessProjectionResult.error,
  };
}) satisfies Load<{ workspaceId: string }>;
