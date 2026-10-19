import { browserUrlSuffix } from "#lib/workspace/auth/routes.ts";
import { error, redirect } from "@sveltejs/kit";
import { loadJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
import { loadWorkspaceRepositoryList } from "#lib/workspace/api/repositories.ts";
import { parseBrowserWorkspaceOrchestratorResponse } from "#lib/workspace/api/workers.ts";
import {
  parseTicketDetail,
  TICKET_BROWSER_API_LOAD_POLICY,
} from "#lib/workspace/api/ticket-browser.ts";
import {
  canonicalResourceReference,
  resourceKey,
} from "#lib/workspace/resource-links.ts";
import type { WorkspaceOrchestratorStatus } from "#lib/workspace/tickets/ticket-panel.ts";
import type { PageLoad } from "./$types";
import { deferPage } from "#lib/workspace/navigation/deferred.ts";

export const load = deferPage(
  async ({ fetch, params, url }: Parameters<PageLoad>[0]) => {
    const reference = resourceKey(params.ticketId);
    const ticketPath = workspaceApiPath(
      params.workspaceId,
      `/tickets/${encodeURIComponent(reference)}`,
    );
    const [ticket, repositoriesRaw, orchestrator] = await Promise.all([
      loadJson(
        fetch,
        ticketPath,
        undefined,
        parseTicketDetail,
        TICKET_BROWSER_API_LOAD_POLICY,
      ),
      loadWorkspaceRepositoryList(fetch, params.workspaceId),
      loadJson<WorkspaceOrchestratorStatus>(
        fetch,
        workspaceApiPath(params.workspaceId, "/orchestrator"),
        undefined,
        parseBrowserWorkspaceOrchestratorResponse,
      ),
    ]);
    if (!ticket.data) {
      const message = ticket.error ?? "Ticket details could not be loaded";
      if (
        ticket.status !== undefined && ticket.status >= 400 &&
        ticket.status <= 599
      ) {
        error(ticket.status, message);
      }
      // Expected read failures belong to the deferred loader, not component construction.
      throw new Error(message);
    }
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
          }${browserUrlSuffix(url)}`,
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
  },
);
