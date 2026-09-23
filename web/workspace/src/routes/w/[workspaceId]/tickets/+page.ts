import { parseBrowserWorkspaceOrchestratorResponse } from "$lib/workspace/api/workers";
import { loadWorkspaceRepositoryList } from "$lib/workspace/api/repositories";
import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import {
  parseTicketListResponse,
  TICKET_BROWSER_API_LOAD_POLICY,
} from "$lib/workspace/api/ticket-browser";
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

async function loadLane(
  fetchFn: typeof fetch,
  workspaceId: string,
  laneId: TicketLaneId,
): Promise<TicketLanePage> {
  const states = LANE_STATES[laneId];
  const search = new URLSearchParams({
    limit: "30",
    states: states.join(","),
  });
  const result = await loadJson(
    fetchFn,
    `/api/w/${encodeURIComponent(workspaceId)}/tickets?${search}`,
    undefined,
    parseTicketListResponse,
    TICKET_BROWSER_API_LOAD_POLICY,
  );
  if (!result.data) {
    throw new Error(result.error ?? `failed to load ${laneId} Ticket lane`);
  }
  return { states: [...states], response: result.data };
}

export const load: PageLoad = async ({ fetch, params }) => {
  const workspaceId = params.workspaceId;
  const [
    readyPlanning,
    inprogressQueued,
    doneClosed,
    orchestrator,
    repositories,
  ] = await Promise.all([
    loadLane(fetch, workspaceId, "ready-planning"),
    loadLane(fetch, workspaceId, "inprogress-queued"),
    loadLane(fetch, workspaceId, "done-closed"),
    loadJson<WorkspaceOrchestratorStatus>(
      fetch,
      workspaceApiPath(workspaceId, "/orchestrator"),
      undefined,
      parseBrowserWorkspaceOrchestratorResponse,
    ),
    loadWorkspaceRepositoryList(fetch, workspaceId),
  ]);

  return {
    workspaceId,
    ticketLanes: {
      "ready-planning": readyPlanning,
      "inprogress-queued": inprogressQueued,
      "done-closed": doneClosed,
    },
    orchestrator,
    repositories,
  };
};
