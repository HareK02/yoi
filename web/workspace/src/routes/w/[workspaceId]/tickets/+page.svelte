<script lang="ts">
  import { formatDate, workspaceRoute } from '$lib/workspace/api/http';
  import type { TicketSummary } from '$lib/workspace/sidebar/types';
  import type { PageProps } from './$types';

  type SortKey = 'title' | 'state' | 'priority' | 'updated_at' | 'queued_at' | 'id';
  type SortDirection = 'asc' | 'desc';

  let { data }: PageProps = $props();

  let query = $state('');
  let stateFilter = $state('all');
  let priorityFilter = $state('all');
  let queuedFilter = $state('all');
  let sortKey = $state<SortKey>('updated_at');
  let sortDirection = $state<SortDirection>('desc');

  const tickets = $derived(data.tickets.data?.items ?? []);
  const states = $derived(uniqueValues(tickets.map((ticket) => ticket.state)));
  const priorities = $derived(uniqueValues(tickets.map((ticket) => ticket.priority).filter(isPresent)));
  const filteredTickets = $derived(filterTickets(tickets));
  const visibleTickets = $derived(sortTickets(filteredTickets));

  function isPresent(value: string | null | undefined): value is string {
    return Boolean(value && value.trim());
  }

  function uniqueValues(values: string[]): string[] {
    return [...new Set(values.filter((value) => value.trim()))].sort((left, right) =>
      left.localeCompare(right),
    );
  }

  function filterTickets(items: TicketSummary[]): TicketSummary[] {
    const needle = query.trim().toLowerCase();
    return items.filter((ticket) => {
      if (stateFilter !== 'all' && ticket.state !== stateFilter) {
        return false;
      }
      if (priorityFilter !== 'all' && (ticket.priority ?? '') !== priorityFilter) {
        return false;
      }
      if (queuedFilter === 'queued' && !ticket.queued_at) {
        return false;
      }
      if (queuedFilter === 'unqueued' && ticket.queued_at) {
        return false;
      }
      if (!needle) {
        return true;
      }
      return [ticket.id, ticket.title, ticket.state, ticket.priority, ticket.queued_by, ticket.record_source]
        .filter(isPresent)
        .some((value) => value.toLowerCase().includes(needle));
    });
  }

  function sortTickets(items: TicketSummary[]): TicketSummary[] {
    return [...items].sort((left, right) => {
      const result = compareTicketValues(left, right, sortKey);
      return sortDirection === 'asc' ? result : -result;
    });
  }

  function compareTicketValues(left: TicketSummary, right: TicketSummary, key: SortKey): number {
    if (key === 'updated_at' || key === 'queued_at') {
      return compareDate(left[key], right[key]);
    }
    return compareText(ticketValue(left, key), ticketValue(right, key));
  }

  function ticketValue(ticket: TicketSummary, key: SortKey): string | null | undefined {
    if (key === 'id') return ticket.id;
    if (key === 'title') return ticket.title;
    if (key === 'state') return ticket.state;
    if (key === 'priority') return ticket.priority;
    return null;
  }

  function compareText(left: string | null | undefined, right: string | null | undefined): number {
    const leftText = left?.trim() ?? '';
    const rightText = right?.trim() ?? '';
    if (!leftText && rightText) return 1;
    if (leftText && !rightText) return -1;
    return leftText.localeCompare(rightText);
  }

  function compareDate(left: string | null | undefined, right: string | null | undefined): number {
    const leftTime = left ? Date.parse(left) : Number.NEGATIVE_INFINITY;
    const rightTime = right ? Date.parse(right) : Number.NEGATIVE_INFINITY;
    return leftTime - rightTime;
  }

  function toggleSort(key: SortKey) {
    if (sortKey === key) {
      sortDirection = sortDirection === 'asc' ? 'desc' : 'asc';
      return;
    }
    sortKey = key;
    sortDirection = key === 'updated_at' || key === 'queued_at' ? 'desc' : 'asc';
  }

  function sortLabel(key: SortKey): string {
    if (sortKey !== key) return '';
    return sortDirection === 'asc' ? '↑' : '↓';
  }

  function resetFilters() {
    query = '';
    stateFilter = 'all';
    priorityFilter = 'all';
    queuedFilter = 'all';
  }
</script>

<svelte:head>
  <title>Tickets · Yoi Workspace</title>
  <meta name="description" content="Workspace Tickets" />
</svelte:head>

<section class="card ticket-database-card">
  <div class="detail-heading">
    <div>
      <p class="eyebrow">Workspace records</p>
      <h2>Tickets</h2>
    </div>
    {#if data.tickets.data}
      <span>{visibleTickets.length} / {data.tickets.data.items.length} ticket{data.tickets.data.items.length === 1 ? '' : 's'}</span>
    {/if}
  </div>

  <p class="section-note">
    Tickets are read from the typed Ticket backend. This read-only table supports Notion-style filtering and sorting for browsing imported workspace Tickets.
  </p>

  {#if data.tickets.data}
    {#if data.tickets.data.items.length === 0}
      <p>No Ticket records are present.</p>
    {:else}
      <div class="ticket-database-toolbar" aria-label="Ticket table controls">
        <label class="ticket-filter ticket-search">
          <span>Search</span>
          <input bind:value={query} type="search" placeholder="Title, id, state, source…" />
        </label>
        <label class="ticket-filter">
          <span>State</span>
          <select bind:value={stateFilter}>
            <option value="all">All states</option>
            {#each states as state}
              <option value={state}>{state}</option>
            {/each}
          </select>
        </label>
        <label class="ticket-filter">
          <span>Priority</span>
          <select bind:value={priorityFilter}>
            <option value="all">All priorities</option>
            {#each priorities as priority}
              <option value={priority}>{priority}</option>
            {/each}
          </select>
        </label>
        <label class="ticket-filter">
          <span>Queue</span>
          <select bind:value={queuedFilter}>
            <option value="all">All</option>
            <option value="queued">Queued</option>
            <option value="unqueued">Unqueued</option>
          </select>
        </label>
        <button class="secondary-button" type="button" onclick={resetFilters}>Reset</button>
      </div>

      <div class="ticket-table-wrap" aria-label="Workspace Tickets table">
        <table class="ticket-table">
          <thead>
            <tr>
              <th><button type="button" onclick={() => toggleSort('title')}>Title {sortLabel('title')}</button></th>
              <th><button type="button" onclick={() => toggleSort('state')}>State {sortLabel('state')}</button></th>
              <th><button type="button" onclick={() => toggleSort('priority')}>Priority {sortLabel('priority')}</button></th>
              <th><button type="button" onclick={() => toggleSort('updated_at')}>Updated {sortLabel('updated_at')}</button></th>
              <th><button type="button" onclick={() => toggleSort('queued_at')}>Queued {sortLabel('queued_at')}</button></th>
              <th><button type="button" onclick={() => toggleSort('id')}>ID {sortLabel('id')}</button></th>
            </tr>
          </thead>
          <tbody>
            {#each visibleTickets as ticket (ticket.id)}
              <tr>
                <td class="ticket-title-cell">
                  <a href={workspaceRoute(data.workspaceId, `/tickets/${ticket.id}`)}>{ticket.title}</a>
                  <span>{ticket.record_source ?? 'ticket backend'}</span>
                </td>
                <td><span class="state-pill">{ticket.state}</span></td>
                <td>{ticket.priority || '—'}</td>
                <td>{ticket.updated_at ? formatDate(ticket.updated_at) : 'unknown'}</td>
                <td>
                  {#if ticket.queued_at}
                    <span>{formatDate(ticket.queued_at)}</span>
                    {#if ticket.queued_by}
                      <small>{ticket.queued_by}</small>
                    {/if}
                  {:else}
                    <span class="muted">—</span>
                  {/if}
                </td>
                <td><code>{ticket.id}</code></td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>

      {#if visibleTickets.length === 0}
        <p class="section-note">No tickets match the current filters.</p>
      {/if}
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

<style>
  .ticket-database-card {
    overflow: hidden;
  }

  .ticket-database-toolbar {
    display: grid;
    grid-template-columns: minmax(16rem, 1.8fr) repeat(3, minmax(9rem, 1fr)) auto;
    gap: 0.75rem;
    align-items: end;
    margin: 1rem 0;
  }

  .ticket-filter {
    display: grid;
    gap: 0.35rem;
    color: var(--muted-foreground, #667085);
    font-size: 0.78rem;
    font-weight: 600;
    letter-spacing: 0.02em;
    text-transform: uppercase;
  }

  .ticket-filter input,
  .ticket-filter select {
    min-height: 2.35rem;
    border: 1px solid var(--border, #d0d5dd);
    border-radius: 0.6rem;
    background: var(--surface, #fff);
    color: inherit;
    font: inherit;
    font-weight: 500;
    letter-spacing: normal;
    padding: 0 0.7rem;
    text-transform: none;
  }

  .secondary-button {
    min-height: 2.35rem;
    border: 1px solid var(--border, #d0d5dd);
    border-radius: 0.6rem;
    background: var(--surface, #fff);
    color: inherit;
    cursor: pointer;
    font: inherit;
    font-weight: 600;
    padding: 0 0.9rem;
  }

  .ticket-table-wrap {
    border: 1px solid var(--border, #d0d5dd);
    border-radius: 0.8rem;
    overflow: auto;
  }

  .ticket-table {
    width: 100%;
    min-width: 58rem;
    border-collapse: collapse;
    font-size: 0.9rem;
  }

  .ticket-table th,
  .ticket-table td {
    border-bottom: 1px solid var(--border, #eaecf0);
    padding: 0.72rem 0.8rem;
    text-align: left;
    vertical-align: top;
  }

  .ticket-table th {
    background: var(--subtle-surface, #f9fafb);
    color: var(--muted-foreground, #667085);
    font-size: 0.76rem;
    letter-spacing: 0.04em;
    position: sticky;
    text-transform: uppercase;
    top: 0;
    z-index: 1;
  }

  .ticket-table th button {
    all: unset;
    cursor: pointer;
  }

  .ticket-table tbody tr:hover {
    background: var(--hover-surface, #f9fafb);
  }

  .ticket-title-cell {
    min-width: 22rem;
  }

  .ticket-title-cell a {
    color: inherit;
    display: block;
    font-weight: 700;
    text-decoration: none;
  }

  .ticket-title-cell a:hover {
    text-decoration: underline;
  }

  .ticket-title-cell span,
  .ticket-table small,
  .muted {
    color: var(--muted-foreground, #667085);
    display: block;
    font-size: 0.78rem;
    margin-top: 0.2rem;
  }

  @media (max-width: 900px) {
    .ticket-database-toolbar {
      grid-template-columns: 1fr;
    }
  }
</style>
