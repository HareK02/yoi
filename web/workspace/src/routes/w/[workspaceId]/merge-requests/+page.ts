import type { PageLoad } from "./$types";
import {
  mergeRequestCollectionPath,
  type MergeRequestListPage,
} from "$lib/workspace/api/merge-requests";

export const load: PageLoad = async ({ params, fetch }) => {
  try {
    const response = await fetch(
      `${mergeRequestCollectionPath(params.workspaceId)}?limit=100`,
    );
    const body = await response.json().catch(() => ({}));
    if (!response.ok) {
      return {
        workspaceId: params.workspaceId,
        mergeRequests: null,
        error: body?.error ?? body?.message ??
          `Request failed (${response.status})`,
      };
    }
    return {
      workspaceId: params.workspaceId,
      mergeRequests: body as MergeRequestListPage,
      error: null,
    };
  } catch (error) {
    return {
      workspaceId: params.workspaceId,
      mergeRequests: null,
      error: error instanceof Error ? error.message : String(error),
    };
  }
};
