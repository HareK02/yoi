import type { PageLoad } from "./$types";
import { loadJson } from "$lib/workspace/api/http";

export interface RepositorySshCredential {
  credential_id: string;
  workspace_id: string;
  name: string;
  public_key_algorithm: string;
  public_key_fingerprint: string;
  current_revision: number;
  status: string;
  created_at: string;
  rotated_at: string | null;
  referenced_repositories: string[];
}

export interface RepositorySshHostTrust {
  host_trust_id: string;
  workspace_id: string;
  hostname: string;
  port: number;
  key_algorithm: string;
  host_key: string;
  fingerprint: string;
  current_revision: number;
  created_at: string;
  updated_at: string;
  referenced_repositories: string[];
}

export const load: PageLoad = async ({ fetch, params }) => {
  const base = `/api/w/${
    encodeURIComponent(params.workspaceId)
  }/settings/repository-access`;
  const [credentialResult, hostTrustResult] = await Promise.all([
    loadJson<RepositorySshCredential[]>(fetch, `${base}/credentials`),
    loadJson<RepositorySshHostTrust[]>(fetch, `${base}/host-trusts`),
  ]);
  if (!credentialResult.data || !hostTrustResult.data) {
    throw new Error(
      credentialResult.error ?? hostTrustResult.error ??
        "Repository access settings unavailable",
    );
  }
  return {
    workspaceId: params.workspaceId,
    credentials: credentialResult.data,
    hostTrusts: hostTrustResult.data,
  };
};
