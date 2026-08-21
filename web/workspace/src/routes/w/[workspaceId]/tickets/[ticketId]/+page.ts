import { redirect } from "@sveltejs/kit";
import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import {
  canonicalResourceReference,
  resourceHumanKey,
} from "$lib/workspace/resource-links";
import type { WorkspaceOrchestratorStatus } from "$lib/workspace/tickets/ticket-panel";
import type {
  RepositoryListResponse,
  TicketDetail,
} from "$lib/workspace/sidebar/types";
import type { PageLoad } from "./$types";

export const load = (async ({ fetch, params }) => {
  const reference = resourceHumanKey(params.ticketId);
  const ticketPath = workspaceApiPath(
    params.workspaceId,
    `/tickets/${encodeURIComponent(reference)}`,
  );
  const [ticket, repositories, orchestrator] = await Promise.all([
    loadJson<TicketDetail>(fetch, ticketPath),
    loadJson<RepositoryListResponse>(
      fetch,
      workspaceApiPath(params.workspaceId, "/repositories"),
    ),
    loadJson<WorkspaceOrchestratorStatus>(
      fetch,
      workspaceApiPath(params.workspaceId, "/orchestrator"),
    ),
  ]);
  if (ticket.data) {
    const canonical = canonicalResourceReference(
      ticket.data.human_key,
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
  return {
    workspaceId: params.workspaceId,
    ticketId: ticket.data?.id ?? reference,
    ticket,
    repositories,
    orchestrator,
  };
}) satisfies PageLoad;
