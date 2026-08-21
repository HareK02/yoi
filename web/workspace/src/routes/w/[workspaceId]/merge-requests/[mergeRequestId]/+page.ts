import type { PageLoad } from "./$types";
import {
  type MergeRequestDetail,
  mergeRequestDetailPath,
} from "$lib/workspace/api/merge-requests";

export const load: PageLoad = async ({ params, fetch }) => {
  try {
    const response = await fetch(
      `${
        mergeRequestDetailPath(params.workspaceId, params.mergeRequestId)
      }?limit=100`,
    );
    const body = await response.json().catch(() => ({}));
    if (!response.ok) {
      return {
        workspaceId: params.workspaceId,
        mergeRequestId: params.mergeRequestId,
        mergeRequest: null,
        error: body?.error ?? body?.message ??
          `Request failed (${response.status})`,
      };
    }
    return {
      workspaceId: params.workspaceId,
      mergeRequestId: params.mergeRequestId,
      mergeRequest: body as MergeRequestDetail,
      error: null,
    };
  } catch (error) {
    return {
      workspaceId: params.workspaceId,
      mergeRequestId: params.mergeRequestId,
      mergeRequest: null,
      error: error instanceof Error ? error.message : String(error),
    };
  }
};
