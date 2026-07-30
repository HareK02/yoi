<script lang="ts">
  import { untrack } from "svelte";
  import type { ApiResult } from "$lib/workspace/api/http";
  import { ticketLanes } from "$lib/workspace/tickets/ticket-panel";
  import type {
    TicketListResponse,
    TicketSummary,
  } from "$lib/workspace/sidebar/types";

  const { data } = $props<{
    data: {
      workspaceId: string;
      tickets: ApiResult<TicketListResponse>;
    };
  }>();

  const initialTickets = untrack(() => data.tickets.data?.items ?? []);
  let tickets = $state<TicketSummary[]>(initialTickets);
  let lanes = $derived(ticketLanes(tickets));

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
    <div class="ticket-panel-summary" aria-label="Ticket summary">
      <strong>{tickets.length}</strong>
      <span>tickets</span>
    </div>
  </header>

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
