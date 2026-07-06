import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ params, parent }) => {
  const layout = await parent();
  return {
    workspaceId: layout.workspace?.workspace_id ?? "",
    runtimeId: params.runtimeId,
    workerId: params.workerId,
  };
};
