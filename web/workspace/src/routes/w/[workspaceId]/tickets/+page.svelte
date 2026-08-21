<script lang="ts">
  import { untrack } from "svelte";
  import type { ApiResult } from "$lib/workspace/api/http";
  import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
  import type {
    QueryPage,
    TicketListResponse,
    TicketSummary,
  } from "$lib/generated/ticket-api";
  import { ticketHref } from "$lib/workspace/resource-links";
  import {
    ticketLanes,
    type WorkspaceOrchestratorStatus,
  } from "$lib/workspace/tickets/ticket-panel";
  import type { PageData } from "./$types";

  type LaneState = {
    states: string[];
    tickets: TicketSummary[];
    page: QueryPage;
    loading: boolean;
    error: string | null;
  };

  let { data }: { data: PageData } = $props();
  // svelte-ignore state_referenced_locally
  let laneState = $state<Record<string, LaneState>>(
    Object.fromEntries(
      Object.entries(data.ticketLanes).map(([laneId, lane]) => [
        laneId,
        {
          states: [...lane.states],
          tickets: lane.response.items,
          page: lane.response.page,
          loading: false,
          error: null,
        },
      ]),
    ),
  );
  let orchestrator = $state<ApiResult<WorkspaceOrchestratorStatus>>(
    untrack(() => data.orchestrator),
  );
  let orchestratorStarting = $state(false);
  const tickets = $derived(
    Object.values(laneState).flatMap((lane) => lane.tickets),
  );
  const lanes = $derived(ticketLanes(tickets));

  function mergeTickets(
    current: TicketSummary[],
    incoming: TicketSummary[],
  ): TicketSummary[] {
    const byId = new Map(current.map((ticket) => [ticket.id, ticket]));
    for (const ticket of incoming) byId.set(ticket.id, ticket);
    return [...byId.values()];
  }

  async function loadMore(laneId: string): Promise<void> {
    const lane = laneState[laneId];
    if (!lane || lane.loading || !lane.page.has_more || !lane.page.next_cursor) {
      return;
    }
    lane.loading = true;
    lane.error = null;
    try {
      const search = new URLSearchParams({
        limit: "30",
        states: lane.states.join(","),
        cursor: lane.page.next_cursor,
      });
      const response = await fetch(
        `/api/w/${encodeURIComponent(data.workspaceId)}/tickets?${search}`,
      );
      if (!response.ok) {
        throw new Error(`追加読み込みに失敗しました (${response.status})`);
      }
      const page = (await response.json()) as TicketListResponse;
      lane.tickets = mergeTickets(lane.tickets, page.items);
      lane.page = page.page;
    } catch (error) {
      lane.error = error instanceof Error ? error.message : String(error);
    } finally {
      lane.loading = false;
    }
  }

  function handleLaneScroll(event: Event, laneId: string): void {
    const container = event.currentTarget as HTMLElement;
    const remaining =
      container.scrollHeight - container.scrollTop - container.clientHeight;
    if (remaining <= 96) void loadMore(laneId);
  }

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
</script>

<svelte:head>
  <title>Tickets · {data.workspaceId}</title>
</svelte:head>

<div class="workspace-page ticket-panel-page">
  <header class="workspace-page-header ticket-panel-header">
    <div>
      <p class="workspace-eyebrow">Delivery</p>
      <h1>Tickets</h1>
      <p class="workspace-page-lede">
        Plan, route, review, and close work without leaving the workspace.
      </p>
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
        <strong>{tickets.length}</strong>
        <span>loaded tickets</span>
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
      {@const pagination = laneState[lane.id]}
      <section class="ticket-lane" data-state={lane.id}>
        <header class="ticket-lane-header">
          <div>
            <span class="ticket-state-dot"></span>
            <h2>{lane.label}</h2>
          </div>
          <span class="ticket-lane-count">{lane.tickets.length}</span>
        </header>
        <div
          class="ticket-lane-cards"
          data-lane-id={lane.id}
          onscroll={(event) => handleLaneScroll(event, lane.id)}
        >
          {#each lane.tickets as ticket (ticket.id)}
            <a
              class="ticket-card"
              href={ticketHref(data.workspaceId, ticket)}
            >
              <span class="ticket-card-id">{ticket.resource_key}</span>
              <strong>{ticket.title}</strong>
              <div class="ticket-card-meta">
                <span>{ticket.state} · {ticket.priority}</span>
                <time>{prettyDate(ticket.updated_at)}</time>
              </div>
            </a>
          {:else}
            <div class="ticket-lane-empty">No tickets</div>
          {/each}
          {#if pagination?.loading}
            <p class="ticket-lane-page-state" aria-live="polite">Loading…</p>
          {:else if pagination?.error}
            <div class="ticket-lane-page-state ticket-lane-page-error" role="alert">
              <span>{pagination.error}</span>
              <button type="button" onclick={() => loadMore(lane.id)}>Retry</button>
            </div>
          {:else if pagination && !pagination.page.has_more && lane.tickets.length > 0}
            <p class="ticket-lane-page-state">End of lane</p>
          {/if}
        </div>
      </section>
    {/each}
  </section>
</div>
