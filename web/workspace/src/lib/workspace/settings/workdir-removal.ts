import type { WorkingDirectoryRemovalResponse } from "#lib/generated/workdir-api.ts";
import type {
  CleanupWorkdirCandidate,
  WorkingDirectorySummary,
} from "#lib/workspace/sidebar/types.ts";
import { workspaceApiPath } from "#lib/workspace/api/http.ts";
import { parseWorkingDirectoryRemovalResponse } from "#lib/workspace/api/workdirs.ts";

// Only reviewed category codes become public copy. Never render a provider message,
// category verbatim, OS error, or host path (including for unknown future codes).
const causes: Readonly<Record<string, string>> = {
  mount_present:
    "A mounted filesystem remains inside this Workdir. Ask its resource owner to verify ownership and safely release it before retrying. Unknown or shared mounts must not be forcibly unmounted.",
  mount_check_unavailable:
    "The Runtime could not check for mounted filesystems. Restore mount inspection on the Runtime before retrying.",
  permission_denied:
    "Removal permission was denied. Ask the Workspace or Runtime administrator to verify the required access; do not bypass protection with a forced deletion.",
  resource_busy:
    "A resource is still in use. Ask its owner to stop using it and release it safely before retrying.",
  storage_unavailable:
    "Runtime storage is unavailable. Restore storage access and capacity before retrying.",
  ownership_unknown:
    "Resource ownership could not be verified. Ask the Runtime administrator to verify ownership before any resource is released or removed.",
  changes_present:
    "This Workdir has changes. Preserve or clean those changes before requesting removal again.",
  changes_unknown:
    "Current changes could not be checked. Restore access so the Runtime can verify change protection before removal.",
  provider_cleanup_failed:
    "The provider could not finish removal. Ask the Runtime administrator to inspect the correlated removal diagnostics and resolve the cause before retrying.",
  provider_observation_unknown:
    "The provider could not confirm the remaining Workdir. Restore provider access so the next removal can recheck its current contents.",
  provider_cleanup_outcome_unknown:
    "The provider has not confirmed deletion. Restore provider access and retry the same removal request to recheck the remaining target. The Workdir stays listed until deletion is confirmed.",
  authority_invalid:
    "The removal authority could not be verified. Ask the Workspace administrator to verify the Workdir target and owning Runtime before requesting removal again.",
  provider_identity_changed:
    "The provider target identity changed. Ask the Runtime administrator to verify the target; do not delete a replacement resource.",
  runtime_unavailable:
    "The Runtime is unavailable. Restore its connection before retrying removal.",
  provider_unavailable:
    "The Workdir provider is unavailable. Restore its connection before retrying removal.",
  unsupported_target:
    "This target does not support ordinary removal. Ask its resource owner for the supported cleanup operation.",
  blocked_by_live_authority:
    "This Workdir has an active attachment, reservation, or hold. Release it through its owning operation before requesting removal again.",
  dirty_or_unknown:
    "Current change protection prevents removal. Preserve changes or restore change inspection before requesting removal again.",
  authority_changed:
    "Removal authority or current usage changed. Refresh the inventory and resolve current attachments, reservations, or holds before requesting removal again.",
};
const unknownCause =
  "Removal could not be confirmed. Check the Runtime's correlated removal diagnostics and resolve the cause before requesting removal again. The Workdir remains listed until deletion is confirmed.";

export function removalCause(category: string | null | undefined): string {
  return category != null && Object.hasOwn(causes, category)
    ? causes[category]
    : unknownCause;
}

export function canRemoveWorkdir(
  workdir: WorkingDirectorySummary,
  candidate: CleanupWorkdirCandidate | undefined,
): boolean {
  return Boolean(
    candidate && !candidate.blocking_reason && !candidate.running_linked &&
      !candidate.pinned_linked &&
      (candidate.action === "workdir_clean_cleanup" ||
        candidate.action === "workdir_record_delete") &&
      workdir.cleanliness !== "dirty" && !workdir.occupied_by,
  );
}

export function removalGuard(
  workdir: WorkingDirectorySummary,
  candidate: CleanupWorkdirCandidate | undefined,
): string {
  if (
    workdir.occupied_by || candidate?.running_linked ||
    candidate?.pinned_linked || candidate?.blocking_reason
  ) {
    return "An attachment, reservation, or hold protects this Workdir. Release it through its owning operation, then refresh.";
  }
  if (
    workdir.cleanliness === "dirty" ||
    candidate?.action === "workdir_dirty_discard"
  ) {
    return "Changes are protected. Preserve or clean them, then refresh before deletion.";
  }
  return "Removal eligibility is unavailable. Refresh before deletion.";
}

export class WorkdirRemovalError extends Error {
  constructor(readonly category?: string) {
    super(removalCause(category));
  }
}

// Read only a small public DTO. Discard HTTP error bodies without interpreting or
// displaying them, and drain small bodies rather than aborting completed requests.
async function removalBody(response: Response): Promise<string> {
  const reader = response.body?.getReader();
  if (!reader) return "";
  const chunks: Uint8Array[] = [];
  let bytes = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      bytes += value.byteLength;
      if (bytes > 4096) {
        await reader.cancel();
        throw new WorkdirRemovalError();
      }
      chunks.push(value);
    }
    const body = new Uint8Array(bytes);
    let offset = 0;
    for (const chunk of chunks) {
      body.set(chunk, offset);
      offset += chunk.byteLength;
    }
    return new TextDecoder().decode(body);
  } finally {
    reader.releaseLock();
  }
}

export async function removeWorkdir(
  fetchFn: typeof fetch,
  workspaceId: string,
  runtimeId: string,
  workdirId: string,
): Promise<WorkingDirectoryRemovalResponse> {
  const response = await fetchFn(
    workspaceApiPath(
      workspaceId,
      `/runtimes/${encodeURIComponent(runtimeId)}/working-directories/${
        encodeURIComponent(workdirId)
      }`,
    ),
    {
      method: "DELETE",
      headers: { "content-type": "application/json" },
      // An ordinary repeated request uses the existing durable Backend removal authority.
      body: JSON.stringify({ reason: "User requested Workdir deletion" }),
    },
  );
  if (!response.ok) {
    await removalBody(response).catch(() => undefined);
    throw new WorkdirRemovalError(
      response.status === 403
        ? "permission_denied"
        : response.status === 409
        ? "authority_changed"
        : response.status === 503
        ? "provider_unavailable"
        : undefined,
    );
  }
  const result = parseWorkingDirectoryRemovalResponse(
    JSON.parse(await removalBody(response)),
  );
  if (result.working_directory_id !== workdirId) {
    throw new WorkdirRemovalError();
  }
  return result;
}
