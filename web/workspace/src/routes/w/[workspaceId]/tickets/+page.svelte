<script lang="ts">
  import { untrack } from "svelte";
  import type { ApiResult } from "$lib/workspace/api/http";
  import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
  import {
    ticketLanes,
    type WorkspaceOrchestratorStatus,
  } from "$lib/workspace/tickets/ticket-panel";
  import type {
    TicketListResponse,
    TicketSummary,
  } from "$lib/workspace/sidebar/types";

  const { data } = $props<{
    data: {
      workspaceId: string;
      tickets: ApiResult<TicketListResponse>;
      orchestrator: ApiResult<WorkspaceOrchestratorStatus>;
    };
  }>();

  const initialTickets = untrack(() => data.tickets.data?.items ?? []);
  let tickets = $state<TicketSummary[]>(initialTickets);
  let orchestrator = $state<ApiResult<WorkspaceOrchestratorStatus>>(
    untrack(() => data.orchestrator),
  );
  let orchestratorStarting = $state(false);
  let lanes = $derived(ticketLanes(tickets));

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

<svelte:head><title>Tickets · Yoi</title></svelte:head>

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
        <span>tickets</span>
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
          <span class="ticket-lane-count">{lane.tickets.length}</span>
        </header>

        <div class="ticket-lane-cards">
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
            <div class="ticket-lane-empty">No tickets</div>
          {/each}
        </div>
      </section>
    {/each}
  </section>
</div>
