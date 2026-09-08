import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import { parseWorkspaceRuntimeList } from "$lib/workspace/api/runtime-management";
import { parseWorkspaceSigningIdentityResponse } from "$lib/workspace/settings/profile-api";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const runtimes = await loadJson(
    fetch,
    workspaceApiPath(params.workspaceId, "/runtimes"),
    undefined,
    (value) => {
      const response = parseWorkspaceRuntimeList(value);
      if (response.workspace_id !== params.workspaceId) {
        throw new Error("Runtime list Workspace did not match the route");
      }
      return response;
    },
  );

  const signingIdentity = await loadJson(
    fetch,
    workspaceApiPath(params.workspaceId, "/settings/signing-identity"),
    undefined,
    (value) => {
      const response = parseWorkspaceSigningIdentityResponse(value);
      if (response.identity.workspace_id !== params.workspaceId) {
        throw new Error("Workspace signing identity did not match the route");
      }
      return response;
    },
  );

  return {
    workspaceId: params.workspaceId,
    runtimes: runtimes.data,
    runtimesError: runtimes.error,
    signingIdentity: signingIdentity.data,
    signingIdentityError: signingIdentity.error,
  };
};
