import { redirect } from "@sveltejs/kit";
import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import {
  canonicalResourceReference,
  resourceHumanKey,
} from "$lib/workspace/resource-links";
import type { WorkspaceOrchestratorStatus } from "$lib/workspace/tickets/ticket-panel";
import type { RepositoryListResponse, TicketDetail } from "$lib/workspace/sidebar/types";
import type { PageLoad } from "./$types";

async function loadOptionalJson<T>(fetcher: typeof fetch, path: string): Promise<{ data: T | null; error: string | null }> {
  try {
    const response = await fetcher(path);
    if (response.status === 404) return { data: null, error: null };
    if (!response.ok) return { data: null, error: await response.text() || `HTTP ${response.status}` };
    return { data: await response.json() as T, error: null };
  } catch (error) {
    return { data: null, error: error instanceof Error ? error.message : String(error) };
  }
}

export const load = (async ({ fetch, params }) => {
  const reference = resourceHumanKey(params.ticketId);
  const ticketPath = workspaceApiPath(params.workspaceId, `/tickets/${encodeURIComponent(reference)}`);
  const [ticket, repositories, orchestrator, mergeRequest] = await Promise.all([
    loadJson<TicketDetail>(fetch, ticketPath),
    loadJson<RepositoryListResponse>(fetch, workspaceApiPath(params.workspaceId, "/repositories")),
    loadJson<WorkspaceOrchestratorStatus>(fetch, workspaceApiPath(params.workspaceId, "/orchestrator")),
    loadOptionalJson<Record<string, unknown>>(fetch, `${ticketPath}/merge-request`),
  ]);
  if (ticket.data) {
    const canonical = canonicalResourceReference(ticket.data.human_key, ticket.data.title);
    if (params.ticketId !== canonical) {
      redirect(308, `/w/${encodeURIComponent(params.workspaceId)}/tickets/${encodeURIComponent(canonical)}`);
    }
  }
  return { workspaceId: params.workspaceId, ticketId: ticket.data?.id ?? reference, ticket, repositories, orchestrator, mergeRequest };
}) satisfies PageLoad;
