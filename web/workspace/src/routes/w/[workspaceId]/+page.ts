import type { PageLoad } from "./$types";

// Home uses the Workspace identity and permissions already loaded by its layout.
// Infrastructure inventories belong to Settings and must not delay task navigation.
export const load: PageLoad = ({ params }) => ({ workspaceId: params.workspaceId });
