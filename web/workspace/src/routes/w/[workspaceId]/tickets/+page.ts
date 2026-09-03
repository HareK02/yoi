import { parseBrowserWorkspaceOrchestratorResponse } from "$lib/workspace/api/workers";
import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import type { TicketListResponse } from "$lib/generated/ticket-api";
import type { WorkspaceOrchestratorStatus } from "$lib/workspace/tickets/ticket-panel";
import type { PageLoad } from "./$types";

const LANE_STATES = {
  "ready-planning": ["ready", "planning"],
  "inprogress-queued": ["inprogress", "queued"],
  "done-closed": ["done", "closed"],
} as const;

export type TicketLaneId = keyof typeof LANE_STATES;

export type TicketLanePage = {
  states: readonly string[];
  response: TicketListResponse;
};

export const load: PageLoad = async ({ fetch, params }) => {
  const workspaceId = params.workspaceId;
  const [entries, orchestrator] = await Promise.all([
    Promise.all(
      Object.entries(LANE_STATES).map(async ([laneId, states]) => {
        const search = new URLSearchParams({
          limit: "30",
          states: states.join(","),
        });
        const response = await fetch(
          `/api/w/${encodeURIComponent(workspaceId)}/tickets?${search}`,
        );
        if (!response.ok) {
          throw new Error(
            `failed to load ${laneId} Ticket lane (${response.status})`,
          );
        }
        return [
          laneId,
          { states: [...states], response: await response.json() },
        ] as const;
      }),
    ),
    loadJson<WorkspaceOrchestratorStatus>(
      fetch,
      workspaceApiPath(workspaceId, "/orchestrator"),
      undefined,
      parseBrowserWorkspaceOrchestratorResponse,
    ),
  ]);

  return {
    workspaceId,
    ticketLanes: Object.fromEntries(entries) as unknown as Record<
      TicketLaneId,
      TicketLanePage
    >,
    orchestrator,
  };
};
