<script lang="ts">
  import type { RepositoryTicketsResponse, TicketSummary } from '$lib/workspace/sidebar/types';

  const INITIAL_VISIBLE_ROWS = 30;
  const VISIBLE_ROW_INCREMENT = 30;
  const NEAR_BOTTOM_PX = 96;

  type KanbanGroup = {
    key: string;
    label: string;
    states: string[];
    items: TicketSummary[];
  };

  type GroupMetadata = {
    key: string;
    label: string;
    states: string[];
  };

  let { tickets }: { tickets: RepositoryTicketsResponse } = $props();

  let visibleRowsByGroup = $state<Record<string, number>>({});
  let groups = $derived(buildGroups(tickets.columns));

  $effect(() => {
    const groupKeys = new Set(groups.map((group) => group.key));
    const nextVisibleRows = { ...visibleRowsByGroup };
    let changed = false;

    for (const group of groups) {
      if (nextVisibleRows[group.key] === undefined) {
        nextVisibleRows[group.key] = INITIAL_VISIBLE_ROWS;
        changed = true;
      }
    }

    for (const key of Object.keys(nextVisibleRows)) {
      if (!groupKeys.has(key)) {
        delete nextVisibleRows[key];
        changed = true;
      }
    }

    if (changed) {
      visibleRowsByGroup = nextVisibleRows;
    }
  });

  function buildGroups(columns: RepositoryTicketsResponse['columns']): KanbanGroup[] {
    const groupsByKey = new Map<string, KanbanGroup>();

    for (const column of columns) {
      const metadata = groupMetadataForState(column.state);
      let group = groupsByKey.get(metadata.key);
      if (!group) {
        group = {
          key: metadata.key,
          label: metadata.label,
          states: metadata.states,
          items: []
        };
        groupsByKey.set(metadata.key, group);
      }
      group.items.push(...column.items);
    }

    return Array.from(groupsByKey.values()).map((group) => ({
      ...group,
      items: sortGroupItems(group.items)
    }));
  }

  function groupMetadataForState(state: string): GroupMetadata {
    if (state === 'planning' || state === 'ready') {
      return {
        key: 'ready-planning',
        label: 'Ready / Planning',
        states: ['ready', 'planning']
      };
    }
    if (state === 'queued' || state === 'inprogress') {
      return {
        key: 'inprogress-queued',
        label: 'In progress / Queued',
        states: ['inprogress', 'queued']
      };
    }
    if (state === 'other') {
      return {
        key: 'state:other',
        label: 'Other states',
        states: ['other']
      };
    }
    return {
      key: `state:${state}`,
      label: state,
      states: [state]
    };
  }

  function sortGroupItems(items: TicketSummary[]): TicketSummary[] {
    return items
      .map((ticket, index) => ({ ticket, index }))
      .sort((left, right) => {
        const stateOrder = statePriority(left.ticket.state) - statePriority(right.ticket.state);
        if (stateOrder !== 0) {
          return stateOrder;
        }
        return left.index - right.index;
      })
      .map(({ ticket }) => ticket);
  }

  function statePriority(state: string): number {
    if (state === 'ready' || state === 'inprogress') {
      return 0;
    }
    if (state === 'planning' || state === 'queued') {
      return 1;
    }
    return 2;
  }

  function visibleCount(groupKey: string): number {
    return visibleRowsByGroup[groupKey] ?? INITIAL_VISIBLE_ROWS;
  }

  function visibleTickets(group: KanbanGroup): TicketSummary[] {
    return group.items.slice(0, visibleCount(group.key));
  }

  function hasMore(group: KanbanGroup): boolean {
    return visibleCount(group.key) < group.items.length;
  }

  function onGroupScroll(group: KanbanGroup, event: Event) {
    if (!hasMore(group)) {
      return;
    }

    const target = event.currentTarget;
    if (!(target instanceof HTMLElement)) {
      return;
    }

    const distanceFromBottom = target.scrollHeight - target.scrollTop - target.clientHeight;
    if (distanceFromBottom > NEAR_BOTTOM_PX) {
      return;
    }

    visibleRowsByGroup = {
      ...visibleRowsByGroup,
      [group.key]: Math.min(group.items.length, visibleCount(group.key) + VISIBLE_ROW_INCREMENT)
    };
  }

  function formatDate(value: string | null | undefined): string {
    return value ?? 'not recorded';
  }
</script>

<div class="repository-ticket-kanban">
  {#each groups as group (group.key)}
    <article class="ticket-group" aria-labelledby={`${group.key}-heading`}>
      <header class="ticket-group-heading">
        <div>
          <h3 id={`${group.key}-heading`}>{group.label}</h3>
          <p>{group.states.join(' + ')}</p>
        </div>
        <span>{group.items.length}</span>
      </header>

      {#if group.items.length === 0}
        <p class="muted">No tickets.</p>
      {:else}
        <div
          class="ticket-list-scroll"
          aria-label={`${group.label} tickets`}
          onscroll={(event) => onGroupScroll(group, event)}
        >
          <ul class="ticket-list">
            {#each visibleTickets(group) as ticket (ticket.id)}
              <li class="ticket-row">
                <div class="ticket-row-heading">
                  <strong>{ticket.title}</strong>
                  <span class="ticket-state">{ticket.state}</span>
                </div>
                <small><code>{ticket.id}</code> · updated {formatDate(ticket.updated_at)}</small>
              </li>
            {/each}
          </ul>
          {#if hasMore(group)}
            <p class="lazy-note">Showing {visibleCount(group.key)} of {group.items.length}; scroll for more.</p>
          {/if}
        </div>
      {/if}
    </article>
  {/each}
</div>

<style>
  .repository-ticket-kanban {
    display: grid;
    gap: var(--space-5);
    grid-template-columns: repeat(auto-fit, minmax(min(240px, 100%), 1fr));
    margin-top: var(--space-4);
  }

  .ticket-group {
    min-width: 0;
    padding: var(--space-4) 0 0;
    border-top: 1px solid var(--line);
  }

  .ticket-group-heading {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--space-3);
    margin-bottom: var(--space-3);
  }

  .ticket-group-heading h3 {
    margin: 0;
    color: var(--text-muted);
    font-size: 0.9rem;
    letter-spacing: 0.07em;
    text-transform: uppercase;
  }

  .ticket-group-heading p,
  .ticket-group-heading span,
  .lazy-note {
    color: var(--text-faint);
    font-size: 0.78rem;
  }

  .ticket-group-heading p,
  .lazy-note {
    margin: var(--space-1) 0 0;
  }

  .ticket-list-scroll {
    max-height: 34rem;
    overflow-y: auto;
    padding-right: var(--space-2);
    scrollbar-gutter: stable;
  }

  .ticket-list {
    display: grid;
    gap: var(--space-3);
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .ticket-row {
    padding-left: var(--space-3);
    border-left: 2px solid var(--line);
  }

  .ticket-row-heading {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--space-3);
  }

  .ticket-row-heading strong {
    color: var(--text-strong);
  }

  .ticket-state {
    flex: 0 0 auto;
    color: var(--text-muted);
    font-size: 0.72rem;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }
</style>
