<script lang="ts">
  import { formatDate, workspaceRoute } from '$lib/workspace/api/http';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
</script>

<svelte:head>
  <title>Tickets · Yoi Workspace</title>
  <meta name="description" content="Workspace Tickets" />
</svelte:head>

<section class="card">
  <div class="detail-heading">
    <div>
      <p class="eyebrow">Workspace records</p>
      <h2>Tickets</h2>
    </div>
    {#if data.tickets.data}
      <span>{data.tickets.data.items.length} ticket{data.tickets.data.items.length === 1 ? '' : 's'}</span>
    {/if}
  </div>

  <p class="section-note">
    Tickets are read from the typed Ticket backend. This surface is read-only; creation and queue operations remain outside the browser UI for now.
  </p>

  {#if data.tickets.data}
    {#if data.tickets.data.items.length === 0}
      <p>No Ticket records are present.</p>
    {:else}
      <div class="ticket-list" aria-label="Workspace Tickets">
        {#each data.tickets.data.items as ticket (ticket.id)}
          <a class="ticket-row" href={workspaceRoute(data.workspaceId, `/tickets/${ticket.id}`)}>
            <div class="ticket-main">
              <div class="ticket-title-row">
                <strong class="ticket-title">{ticket.title}</strong>
                <span class="state-pill">{ticket.state}</span>
              </div>
              <p class="ticket-summary">
                {ticket.priority ? `${ticket.priority} priority` : 'priority unspecified'} · {ticket.record_source ?? 'ticket backend'}
              </p>
            </div>
            <div class="ticket-meta" aria-label="Ticket metadata">
              <span>Updated {ticket.updated_at ? formatDate(ticket.updated_at) : 'unknown'}</span>
              {#if ticket.queued_at}
                <span>Queued {formatDate(ticket.queued_at)}</span>
              {/if}
              <code>{ticket.id}</code>
            </div>
          </a>
        {/each}
      </div>
    {/if}

    {#if data.tickets.data.invalid_records.length > 0}
      <p class="error">{data.tickets.data.invalid_records.length} invalid Ticket record(s) hidden.</p>
    {/if}
  {:else if data.tickets.error}
    <p class="error">{data.tickets.error}</p>
  {:else}
    <p>Waiting for <code>/api/w/{data.workspaceId}/tickets</code>…</p>
  {/if}
</section>
