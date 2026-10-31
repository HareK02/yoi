<script lang="ts">
  import { onDestroy, onMount, untrack } from "svelte";
  import type {
    TicketClosureReason,
    TicketDetail,
  } from "#lib/generated/ticket-api.ts";
  import { createTicketParentClient } from "#lib/workspace/api/ticket-parent.ts";
  import { ticketHref } from "#lib/workspace/resource-links.ts";
  import {
    MAX_VISIBLE_CHILDREN,
    TicketParentController,
  } from "./ticket-parent-controller.ts";

  let { workspaceId, routeTicketId, ticket }: {
    workspaceId: string;
    routeTicketId: string;
    ticket: TicketDetail;
  } = $props();
  const controller = new TicketParentController(
    createTicketParentClient(),
    {
      getItem: (key) => sessionStorage.getItem(key),
      setItem: (key, value) => sessionStorage.setItem(key, value),
      removeItem: (key) => sessionStorage.removeItem(key),
    },
  );
  let mounted = $state(false);
  onMount(() => {
    mounted = true;
  });
  let query = $state("");
  let reference = $state("");
  let selectedId = $state("");
  let searched = $state(false);
  const selected = $derived(
    $controller.candidates.find((item) => item.id === selectedId) ?? null,
  );
  const editBlocked = $derived(
    $controller.saving || $controller.pending !== null ||
      $controller.loadingParent || $controller.parentError !== null ||
      $controller.retentionError !== null,
  );
  const childrenSearch = $derived(
    `/w/${encodeURIComponent(workspaceId)}/tickets?${
      new URLSearchParams({ q: `parent:${ticket.resource_key}` })
    }`,
  );
  let selectedIdentity = "";
  $effect(() => {
    if (!mounted) return;
    const ws = workspaceId;
    const id = routeTicketId;
    const loadedId = ticket.id;
    untrack(() => {
      const identity = JSON.stringify([ws, id, loadedId]);
      if (identity === selectedIdentity) return;
      selectedIdentity = identity;
      query = "";
      reference = "";
      selectedId = "";
      searched = false;
      if (loadedId !== id) {
        controller.suspend();
        return;
      }
      void controller.select(
        ws,
        id,
        ticket.parent_link,
        ticket.action_eligibility.can_set_parent,
      );
    });
  });
  $effect(() => {
    if (!mounted) return;
    controller.setEligibility(ticket.action_eligibility.can_set_parent);
  });
  onDestroy(() => controller.dispose());

  function closureLabel(reason: TicketClosureReason | null): string {
    if (reason === null) return "reason unavailable";
    return typeof reason === "string" ? reason : reason.other;
  }
  function search(byReference = false): void {
    selectedId = "";
    searched = true;
    void controller.search(byReference ? reference : query, byReference);
  }
</script>

{#if $controller.workspaceId === workspaceId && $controller.ticketId === routeTicketId && ticket.id === routeTicketId}
  <section
    class="ticket-detail-section ticket-hierarchy"
    aria-label="Parent and children"
  >
    <div class="ticket-section-heading"><h2>Parent and children</h2></div>
    <h3>Parent</h3>
    {#if $controller.parent?.parent}
      {@const parent = $controller.parent.parent}
      <a class="hierarchy-link" href={ticketHref(workspaceId, parent)}>
        <strong>{parent.resource_key}</strong> {parent.title}
        <span>{parent.state}{parent.state === "closed"
            ? ` · ${closureLabel(parent.closure_reason)}`
            : ""}</span>
      </a>
    {:else if $controller.parent?.observation.parent_ticket_id}
      <p>Parent details unavailable.</p>
    {:else if $controller.parent}
      <p class="workspace-empty-copy">No parent.</p>
    {/if}
    {#if $controller.loadingParent}
      <p role="status">Reading parent…</p>
    {/if}
    {#if $controller.parentError}
      <p class="workspace-callout is-error" role="alert">
        {$controller.parentError}
      </p>
    {/if}
    <button
      class="workspace-secondary-button"
      type="button"
      disabled={$controller.loadingParent || $controller.saving}
      onclick={() => void controller.readParent()}
    >Reread parent</button>
    {#if $controller.retentionError}
      <p class="workspace-callout is-error" role="alert">
        {$controller.retentionError}
      </p>
    {/if}
    {#if $controller.operationError}
      <p class="workspace-callout is-error" role="alert">
        {$controller.operationError}
      </p>
    {/if}
    {#if $controller.saved}
      <p role="status">
        Parent operation saved. Current parent is read separately.
      </p>
    {/if}
    {#if $controller.pending}
      <p>Unresolved parent operation retained. New changes are blocked.</p>
      {#if $controller.canSetParent}
        <button
          class="workspace-secondary-button"
          type="button"
          disabled={$controller.saving || $controller.loadingParent}
          onclick={() => void controller.retry()}
        >Retry exact parent operation</button>
      {/if}
      <button
        class="workspace-secondary-button"
        type="button"
        disabled={$controller.saving || $controller.loadingParent}
        onclick={() => void controller.readParent()}
      >Reread and reconcile parent</button>
      {#if $controller.pending.rejected && $controller.reconciled}
        <button
          class="workspace-secondary-button"
          type="button"
          disabled={$controller.saving}
          onclick={() => controller.useRefreshedObservation()}
        >Use refreshed parent observation</button>
      {:else if $controller.reconciled}
        <p>
          The latest observation does not confirm this operation. Its original key
          and body remain available for exact retry; a different parent alone does
          not prove failure.
        </p>
      {/if}
      <details>
        <summary>Retained parent request</summary>
        <pre>{$controller.pending.operation.body}</pre>
      </details>
    {/if}
    {#if $controller.canSetParent && !$controller.loadingParent && !$controller.parentError && $controller.parent}
      <details>
        <summary>{$controller.parent.observation.parent_ticket_id
            ? "Change or unlink parent"
            : "Set parent"}</summary>
        <form
          class="ticket-control-form"
          onsubmit={(event) => {
            event.preventDefault();
            search();
          }}
        >
          <label>
            Search existing Tickets
            <input
              bind:value={query}
              disabled={editBlocked || $controller.searching}
              required
            />
          </label>
          <button
            class="workspace-secondary-button"
            type="submit"
            disabled={editBlocked || $controller.searching || !query.trim()}
          >Search Tickets</button>
        </form>
        <form
          class="ticket-control-form"
          onsubmit={(event) => {
            event.preventDefault();
            search(true);
          }}
        >
          <label>
            Ticket reference
            <input
              bind:value={reference}
              placeholder="T-804"
              disabled={editBlocked || $controller.searching}
              required
            />
          </label>
          <button
            class="workspace-secondary-button"
            type="submit"
            disabled={editBlocked || $controller.searching || !reference.trim()}
          >Find Ticket by reference</button>
        </form>
        {#if $controller.searching}
          <p role="status">Finding Tickets…</p>
        {/if}
        {#if $controller.searchError}
          <p class="workspace-callout is-error" role="alert">
            {$controller.searchError}
          </p>
        {/if}
        {#if $controller.candidates.length > 0}
          <fieldset disabled={editBlocked}>
            <legend>Choose existing parent</legend>
            {#each $controller.candidates as candidate (candidate.id)}
              <label class="parent-candidate">
                <input
                  type="radio"
                  name="parent-ticket"
                  value={candidate.id}
                  bind:group={selectedId}
                />
                <span>
                  <strong>{candidate.resource_key}</strong> {candidate.title}
                  <small>{candidate.state}{candidate.state === "closed"
                      ? ` · ${closureLabel(candidate.closure_reason)}`
                      : ""}</small>
                </span>
              </label>
            {/each}
          </fieldset>
          <button
            class="workspace-secondary-button"
            type="button"
            disabled={editBlocked || !selected}
            onclick={() =>
              selected && void controller.change(selected, crypto.randomUUID())}
          >Set selected parent</button>
        {:else if searched && !$controller.searching && !$controller.searchError}
          <p class="workspace-empty-copy">No other matching Tickets.</p>
        {/if}
        {#if $controller.candidatesTruncated}
          <p>
            More matches are omitted. Refine the search or use an exact Ticket
            reference.
          </p>
        {/if}
        {#if $controller.parent.observation.parent_ticket_id}
          <button
            class="workspace-secondary-button"
            type="button"
            disabled={editBlocked}
            onclick={() => void controller.change(null, crypto.randomUUID())}
          >Unlink parent</button>
        {/if}
      </details>
    {/if}

    <h3>Direct children</h3>
    {#if $controller.children}
      <ul class="hierarchy-children">
        {#each $controller.children.items as child (child.id)}
          <li>
            <a class="hierarchy-link" href={ticketHref(workspaceId, child)}>
              <strong>{child.resource_key}</strong> {child.title}
              <span>{child.state}{child.state === "closed"
                  ? ` · ${closureLabel(child.closure_reason)}`
                  : ""}</span>
            </a>
          </li>
        {:else}
          <li class="workspace-empty-copy">No direct children.</li>
        {/each}
      </ul>
      {#if $controller.children.has_more}
        <p>
          Showing {$controller.children.items.length} direct children; more are
          omitted.
        </p>
        {#if $controller.children.items.length < MAX_VISIBLE_CHILDREN}
          <button
            class="workspace-secondary-button"
            type="button"
            disabled={$controller.loadingChildren}
            onclick={() => void controller.readChildren(true)}
          >Load more children</button>
        {/if}
      {/if}
    {/if}
    {#if $controller.loadingChildren}
      <p role="status">Reading children…</p>
    {/if}
    {#if $controller.childrenError}
      <p class="workspace-callout is-error" role="alert">
        {$controller.childrenError}
      </p>
    {/if}
    <div class="hierarchy-actions">
      <button
        class="workspace-secondary-button"
        type="button"
        disabled={$controller.loadingChildren}
        onclick={() => void controller.readChildren()}
      >Refresh children</button>
      <a href={childrenSearch}>View children in Ticket search</a>
    </div>
  </section>
{/if}

<style>
  .ticket-hierarchy {
    min-width: 0;
    overflow-wrap: anywhere;
  }
  h3 {
    font-size: var(--font-size-body);
    line-height: var(--line-height-body);
    margin-block: var(--space-4) var(--space-2);
  }
  .hierarchy-link {
    display: block;
    padding-block: var(--space-2);
  }
  .hierarchy-link strong,
  .parent-candidate strong {
    margin-inline-end: var(--space-2);
  }
  .hierarchy-link span,
  .parent-candidate small {
    display: block;
    color: var(--text-muted);
    font-size: var(--font-size-compact);
    line-height: var(--line-height-compact);
  }
  .hierarchy-children {
    list-style: none;
    padding: 0;
    margin-block: var(--space-2);
  }
  .hierarchy-actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-3);
  }
  fieldset {
    min-width: 0;
    border: 0;
    padding: 0;
    margin-block: var(--space-3);
  }
  .parent-candidate {
    display: flex;
    align-items: start;
    gap: var(--space-2);
    margin-block: var(--space-2);
  }
  .parent-candidate input {
    width: auto;
    flex: none;
  }
  pre {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
</style>
