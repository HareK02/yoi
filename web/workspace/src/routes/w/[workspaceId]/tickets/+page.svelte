<script lang="ts">
  import { untrack } from "svelte";
  import type { TicketListResponse } from "$lib/generated/ticket-api";
  import type { ApiResult } from "$lib/workspace/api/http";
  import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
  import {
    nextTicketLaneVisibleCount,
    TICKET_LANE_PAGE_SIZE,
    ticketLanes,
    type TicketLane,
    type TicketLaneId,
    type WorkspaceOrchestratorStatus,
  } from "$lib/workspace/tickets/ticket-panel";
  import "$lib/workspace/styles/tickets.css";

  type VisibleTicketLane = TicketLane & { visibleCount: number };

  const { data } = $props<{
    data: {
      workspaceId: string;
      tickets: ApiResult<TicketListResponse>;
      orchestrator: ApiResult<WorkspaceOrchestratorStatus>;
    };
  }>();

  let lanes = $state<VisibleTicketLane[]>(
    untrack(() =>
      ticketLanes(data.tickets.data?.items ?? []).map((lane) => ({
        ...lane,
        visibleCount: Math.min(TICKET_LANE_PAGE_SIZE, lane.tickets.length),
      }))
    ),
  );
  let orchestrator = $state<ApiResult<WorkspaceOrchestratorStatus>>(
    untrack(() => data.orchestrator),
  );
  let orchestratorStarting = $state(false);
  const displayedTicketCount = $derived(
    lanes.reduce((count, lane) => count + lane.visibleCount, 0),
  );
  const LANE_LOAD_THRESHOLD_PX = 96;

  async function startOrchestrator() {
    if (orchestratorStarting || orchestrator.data?.online) return;
    orchestratorStarting = true;
    orchestrator = await loadJson<WorkspaceOrchestratorStatus>(
      fetch,
      workspaceApiPath(data.workspaceId, "/orchestrator"),
      { method: "POST" },
    );
    orchestratorStarting = false;
  }

  function prettyDate(value?: string | null): string {
    if (!value) return "—";
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? value : date.toLocaleDateString();
  }

  function revealNextTickets(laneId: TicketLaneId): void {
    lanes = lanes.map((lane) =>
      lane.id === laneId
        ? {
          ...lane,
          visibleCount: nextTicketLaneVisibleCount(
            lane.visibleCount,
            lane.tickets.length,
          ),
        }
        : lane
    );
  }

  function handleLaneScroll(event: Event, laneId: TicketLaneId): void {
    const element = event.currentTarget as HTMLElement;
    const distanceFromBottom = element.scrollHeight - element.scrollTop -
      element.clientHeight;
    if (distanceFromBottom <= LANE_LOAD_THRESHOLD_PX) {
      revealNextTickets(laneId);
    }
  }
</script>

<svelte:head><title>Tickets · Yoi</title></svelte:head>

<div class="workspace-page ticket-panel-page">
  <header class="workspace-page-header ticket-panel-header">
    <div>
      <h1>Tickets</h1>
    </div>
    <div class="ticket-panel-controls">
      <div class="orchestrator-status" data-online={orchestrator.data?.online ?? false}>
        <span class="orchestrator-status-dot"></span>
        <div>
          <strong>Orchestrator</strong>
          <span>{orchestrator.data?.online ? "Online" : "Offline"}</span>
        </div>
        {#if !orchestrator.data?.online}
          <button
            class="workspace-primary-button"
            type="button"
            disabled={orchestratorStarting}
            onclick={startOrchestrator}
          >
            {orchestratorStarting ? "Starting…" : "Start Orchestrator"}
          </button>
        {/if}
      </div>
      <div class="ticket-panel-summary" aria-label="Ticket summary">
        <strong>{displayedTicketCount}</strong>
        <span>tickets displayed</span>
      </div>
    </div>
  </header>

  {#if data.tickets.error}
    <p class="workspace-callout is-error">Tickets: {data.tickets.error}</p>
  {/if}

  {#if orchestrator.error}
    <p class="workspace-callout is-error">
      Orchestrator status: {orchestrator.error}
    </p>
  {:else if !orchestrator.data?.online}
    <p class="workspace-callout">
      Orchestration actions are unavailable until the embedded Orchestrator is online.
    </p>
  {/if}

  <section class="ticket-kanban" aria-label="Ticket workflow board">
    {#each lanes as lane (lane.id)}
      {@const displayedTickets = lane.tickets.slice(0, lane.visibleCount)}
      {@const hasMore = lane.visibleCount < lane.tickets.length}
      <section class="ticket-lane" data-state={lane.id}>
        <header class="ticket-lane-header">
          <div>
            <span class="ticket-state-dot"></span>
            <h2>{lane.label}</h2>
          </div>
          <span class="ticket-lane-count">
            {displayedTickets.length}{hasMore ? "+" : ""}
          </span>
        </header>

        <div
          class="ticket-lane-cards"
          onscroll={(event) => handleLaneScroll(event, lane.id)}
        >
          {#each displayedTickets as ticket (ticket.id)}
            <a
              class="ticket-card"
              href={`/w/${encodeURIComponent(data.workspaceId)}/tickets/${encodeURIComponent(ticket.id)}`}
            >
              <span class="ticket-card-id">{ticket.id}</span>
              <strong>{ticket.title}</strong>
              <div class="ticket-card-meta">
                <span>{ticket.state} · {ticket.priority}</span>
                <time>{prettyDate(ticket.updated_at)}</time>
              </div>
            </a>
          {:else}
            <div class="ticket-lane-empty">No tickets</div>
          {/each}

          {#if hasMore}
            <p class="ticket-lane-load-status" aria-live="polite">
              Scroll for {Math.min(TICKET_LANE_PAGE_SIZE, lane.tickets.length - lane.visibleCount)} more
            </p>
          {:else if displayedTickets.length > 0}
            <p class="ticket-lane-load-status">All tickets displayed.</p>
          {/if}
        </div>
      </section>
    {/each}
  </section>
</div>
