import { redirect } from "@sveltejs/kit";
import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import { loadWorkspaceRepositoryList } from "$lib/workspace/api/repositories";
import {
  canonicalResourceReference,
  resourceKey,
} from "$lib/workspace/resource-links";
import type { WorkspaceOrchestratorStatus } from "$lib/workspace/tickets/ticket-panel";
import type { TicketDetail } from "$lib/workspace/sidebar/types";
import type { PageLoad } from "./$types";

export const load = (async ({ fetch, params }) => {
  const reference = resourceKey(params.ticketId);
  const ticketPath = workspaceApiPath(
    params.workspaceId,
    `/tickets/${encodeURIComponent(reference)}`,
  );
  const [ticket, repositoriesRaw, orchestrator] = await Promise.all([
    loadJson<TicketDetail>(fetch, ticketPath),
    loadWorkspaceRepositoryList(fetch, params.workspaceId),
    loadJson<WorkspaceOrchestratorStatus>(
      fetch,
      workspaceApiPath(params.workspaceId, "/orchestrator"),
    ),
  ]);
  if (ticket.data) {
    const canonical = canonicalResourceReference(
      ticket.data.resource_key,
      ticket.data.title,
    );
    if (params.ticketId !== canonical) {
      redirect(
        308,
        `/w/${encodeURIComponent(params.workspaceId)}/tickets/${
          encodeURIComponent(canonical)
        }`,
      );
    }
  }
  const repositories = repositoriesRaw;

  return {
    workspaceId: params.workspaceId,
    ticketId: ticket.data?.id ?? reference,
    ticket,
    repositories,
    orchestrator,
  };
}) satisfies PageLoad;
