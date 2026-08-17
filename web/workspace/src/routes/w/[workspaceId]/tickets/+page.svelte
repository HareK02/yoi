<script lang="ts">
  import { untrack } from "svelte";
  import type { TicketQueryResponse } from "$lib/generated/ticket-api";
  import type { ApiResult } from "$lib/workspace/api/http";
  import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
  import {
    appendUniqueTicketSummaries,
    ticketLaneQuery,
    ticketSummaryFromQueryItem,
    type TicketCardSummary,
    type TicketLaneId,
    type TicketState,
    type WorkspaceOrchestratorStatus,
  } from "$lib/workspace/tickets/ticket-panel";
  import "$lib/workspace/styles/tickets.css";

  type LanePage = {
    id: TicketLaneId;
    label: string;
    states: TicketState[];
    tickets: TicketCardSummary[];
    nextCursor: string | null;
    hasMore: boolean;
    error: string | null;
  };

  const { data } = $props<{
    data: {
      workspaceId: string;
      ticketLanePages: LanePage[];
      orchestrator: ApiResult<WorkspaceOrchestratorStatus>;
    };
  }>();

  let lanes = $state<(LanePage & { loading: boolean })[]>(
    untrack(() =>
      data.ticketLanePages.map((lane: LanePage) => ({ ...lane, loading: false }))
    ),
  );
  let orchestrator = $state<ApiResult<WorkspaceOrchestratorStatus>>(
    untrack(() => data.orchestrator),
  );
  let orchestratorStarting = $state(false);
  const displayedTicketCount = $derived(
    lanes.reduce((count, lane) => count + lane.tickets.length, 0),
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

  function updateLane(
    laneId: TicketLaneId,
    update: (lane: LanePage & { loading: boolean }) =>
      LanePage & { loading: boolean },
  ): void {
    lanes = lanes.map((lane) => lane.id === laneId ? update(lane) : lane);
  }

  async function loadMoreTickets(laneId: TicketLaneId): Promise<void> {
    const lane = lanes.find((candidate) => candidate.id === laneId);
    if (!lane || lane.loading || (!lane.hasMore && !lane.error)) return;

    updateLane(laneId, (current) => ({
      ...current,
      loading: true,
      error: null,
    }));

    let result: ApiResult<TicketQueryResponse>;
    try {
      result = await loadJson<TicketQueryResponse>(
        fetch,
        workspaceApiPath(data.workspaceId, "/tickets/query"),
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(ticketLaneQuery(lane, lane.nextCursor)),
        },
      );
    } catch (error) {
      updateLane(laneId, (current) => ({
        ...current,
        loading: false,
        error: error instanceof Error
          ? error.message
          : "Unable to load more Tickets.",
      }));
      return;
    }

    if (!result.data) {
      updateLane(laneId, (current) => ({
        ...current,
        loading: false,
        error: result.error ?? "Unable to load more Tickets.",
      }));
      return;
    }

    const incoming = result.data.items.map(ticketSummaryFromQueryItem);
    updateLane(laneId, (current) => ({
      ...current,
      tickets: appendUniqueTicketSummaries(current.tickets, incoming),
      nextCursor: result.data?.page.next_cursor ?? null,
      hasMore: result.data?.page.has_more ?? false,
      loading: false,
      error: null,
    }));
  }

  function handleLaneScroll(event: Event, laneId: TicketLaneId): void {
    const element = event.currentTarget as HTMLElement;
    const distanceFromBottom = element.scrollHeight - element.scrollTop -
      element.clientHeight;
    if (distanceFromBottom <= LANE_LOAD_THRESHOLD_PX) {
      void loadMoreTickets(laneId);
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
        <span>tickets loaded</span>
      </div>
    </div>
  </header>

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
      <section class="ticket-lane" data-state={lane.id}>
        <header class="ticket-lane-header">
          <div>
            <span class="ticket-state-dot"></span>
            <h2>{lane.label}</h2>
          </div>
          <span class="ticket-lane-count">
            {lane.tickets.length}{lane.hasMore ? "+" : ""}
          </span>
        </header>

        <div
          class="ticket-lane-cards"
          onscroll={(event) => handleLaneScroll(event, lane.id)}
        >
          {#each lane.tickets as ticket (ticket.id)}
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
            {#if !lane.error}
              <div class="ticket-lane-empty">No tickets</div>
            {/if}
          {/each}

          {#if lane.loading}
            <p class="ticket-lane-load-status" aria-live="polite">Loading 30 more…</p>
          {:else if lane.error}
            <div class="ticket-lane-load-error">
              <small>{lane.error}</small>
              <button
                class="workspace-secondary-button"
                type="button"
                onclick={() => loadMoreTickets(lane.id)}
              >Retry</button>
            </div>
          {:else if !lane.hasMore && lane.tickets.length > 0}
            <p class="ticket-lane-load-status">All tickets loaded.</p>
          {/if}
        </div>
      </section>
    {/each}
  </section>
</div>
