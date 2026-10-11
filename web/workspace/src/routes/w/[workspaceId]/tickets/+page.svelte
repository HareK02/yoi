<script lang="ts">
  import { goto } from "$app/navigation";
  import { untrack } from "svelte";
  import type { ApiResult } from "#lib/workspace/api/http.ts";
  import {
    loadJson,
    workspaceApiJsonWithBody,
    workspaceApiPath,
  } from "#lib/workspace/api/http.ts";
  import { parseBrowserWorkspaceOrchestratorResponse } from "#lib/workspace/api/workers.ts";
  import {
    TICKET_BROWSER_API_LOAD_POLICY,
    TICKET_BROWSER_API_MAX_RESPONSE_BYTES,
    parseTicketQueryResponse,
    parseTicketRecordRef,
  } from "#lib/workspace/api/ticket-browser.ts";
  import type {
    NewTicket,
    QueryPage,
    TicketQueryItem as TicketSummary,
    TicketState,
    TicketTarget,
    TicketTargetAccess,
  } from "#lib/generated/ticket-api.ts";
  import { ticketHref } from "#lib/workspace/resource-links.ts";
  import {
    ticketQueryRequest, currentOrchestratorAssignment, assignmentPrincipalLabel, handlingLabel, handlingSummary, closureReasonLabel,
    type WorkspaceOrchestratorStatus,
  } from "#lib/workspace/tickets/ticket-panel.ts";
  import type { PageData } from "./$types";

  type LaneState = {
    states: TicketState[];
    tickets: TicketSummary[];
    page: QueryPage | null;
    loading: boolean;
    error: string | null;
  };

  type EditableTicketTarget = {
    repository_key: string;
    ref_selector: string;
    access: TicketTargetAccess;
  };

  let { data }: { data: PageData } = $props();
  function initialLanes(): Record<string, LaneState> {
    return Object.fromEntries(Object.entries(data.ticketLanes).flatMap(([laneId, result]) => result ? [[laneId, {
      states: [laneId as TicketState], tickets: result.data?.items ?? [], page: result.data?.page ?? null,
      loading: false, error: result.error,
    }]] : []));
  }
  let laneState = $state<Record<string, LaneState>>(untrack(initialLanes));
  let principal = $state(untrack(() => data.filters.principal));
  let closureChoice = $state(untrack(() => data.filters.closureReason));
  let filtersBusy = $state(false);
  $effect(() => { const currentData = data; untrack(() => {
    laneState = initialLanes(); principal = currentData.filters.principal; closureChoice = currentData.filters.closureReason; orchestrator = currentData.orchestrator;
  }); });
  let orchestrator = $state<ApiResult<WorkspaceOrchestratorStatus>>(
    untrack(() => data.orchestrator),
  );
  let orchestratorStarting = $state(false);
  let creatingTicket = $state(false);
  let createBusy = $state(false);
  let createError = $state<string | null>(null);
  let createTitle = $state("");
  let createBody = $state("");
  let createTargets = $state<EditableTicketTarget[]>([]);
  const repositories = $derived(data.repositories.data?.items ?? []);
  const createTargetsSavable = $derived.by(() => {
    const keys = new Set<string>();
    for (const target of createTargets) {
      if (
        !repositories.some((repository) =>
          repository.repository_key === target.repository_key
        ) || keys.has(target.repository_key)
      ) return false;
      keys.add(target.repository_key);
    }
    return true;
  });
  const lanes = $derived(Object.entries(laneState).map(([id, lane]) => ({ id, label: id === "open" ? "Open" : "Closed", tickets: lane.tickets })));

  async function applyFilters(event: SubmitEvent) {
    event.preventDefault();
    const form = event.currentTarget as HTMLFormElement;
    const search = new URLSearchParams();
    for (const [key, value] of new FormData(form)) if (typeof value === "string" && value) search.append(key, value);
    filtersBusy = true;
    try { await goto(`?${search}`); } finally { filtersBusy = false; }
  }

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
    if (!lane || lane.loading || (lane.page && !lane.page.has_more)) {
      return;
    }
    lane.loading = true;
    lane.error = null;
    try {
      const result = await loadJson(
        fetch,
        workspaceApiPath(data.workspaceId, "/tickets/query"),
        { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(ticketQueryRequest(data.filters, lane.states, lane.page?.next_cursor ?? null)) },
        parseTicketQueryResponse,
        TICKET_BROWSER_API_LOAD_POLICY,
      );
      if (!result.data) {
        throw new Error(result.error ?? "追加読み込みに失敗しました");
      }
      lane.tickets = mergeTickets(lane.tickets, result.data.items);
      lane.page = result.data.page;
    } catch (error) {
      lane.error = error instanceof Error ? error.message : String(error);
    } finally {
      lane.loading = false;
    }
  }

  function addCreateTarget(): void {
    createTargets.push({
      repository_key: "",
      ref_selector: "",
      access: createTargets.some((target) => target.access === "read_write")
        ? "read_only"
        : "read_write",
    });
  }

  function removeCreateTarget(index: number): void {
    createTargets.splice(index, 1);
  }

  function normalizedCreateTargets(): TicketTarget[] {
    return createTargets.map((target) => ({
      repository_key: target.repository_key.trim(),
      ref_selector: target.ref_selector.trim() || null,
      access: target.access,
    }));
  }

  async function createTicket(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (createBusy || !createTitle.trim() || !createTargetsSavable) return;
    createBusy = true;
    createError = null;
    const request: NewTicket = {
      title: createTitle.trim(),
      kind: "task",
      priority: "P2",
      labels: [],
      body: createBody,
      risk_flags: [],
      workflow_state: "open",
      targets: normalizedCreateTargets(),
    };
    try {
      const created = await workspaceApiJsonWithBody(
        workspaceApiPath(data.workspaceId, "/tickets"),
        { method: "POST", body: JSON.stringify(request) },
        parseTicketRecordRef,
        TICKET_BROWSER_API_MAX_RESPONSE_BYTES,
      );
      const reference = created.resource_key ?? created.id;
      await goto(
        `/w/${encodeURIComponent(data.workspaceId)}/tickets/${encodeURIComponent(reference)}`,
      );
    } catch (error) {
      createError = error instanceof Error ? error.message : String(error);
    } finally {
      createBusy = false;
    }
  }

  async function startOrchestrator() {
    if (orchestratorStarting || orchestrator.data?.online) return;
    orchestratorStarting = true;
    orchestrator = await loadJson<WorkspaceOrchestratorStatus>(
      fetch,
      workspaceApiPath(data.workspaceId, "/orchestrator"),
      { method: "POST" },
      parseBrowserWorkspaceOrchestratorResponse,
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
      <h1>Tickets</h1>
    </div>
    <div class="ticket-panel-controls">
      <button
        class="workspace-primary-button"
        type="button"
        onclick={() => creatingTicket = !creatingTicket}
      >{creatingTicket ? "Cancel" : "Add Ticket"}</button>
      <div class="orchestrator-status" data-online={orchestrator.data?.online ?? false}>
        <span class="orchestrator-status-dot"></span>
        <div>
          <strong>Orchestrator</strong>
          <span>{orchestrator.data ? orchestrator.data.online ? "Online" : "Offline" : "Unavailable"}</span>
        </div>
        {#if orchestrator.data && !orchestrator.data.online}
          <button
            class="workspace-secondary-button"
            type="button"
            disabled={orchestratorStarting}
            onclick={startOrchestrator}
          >
            {orchestratorStarting ? "Starting…" : "Start Orchestrator"}
          </button>
        {/if}
      </div>
    </div>
  </header>

  {#if creatingTicket}
    <form class="ticket-editor ticket-create-form" onsubmit={createTicket}>
      <h2>New Ticket</h2>
      {#if createError}<p class="workspace-callout is-error" role="alert">{createError}</p>{/if}
      {#if data.repositories.error}
        <p class="workspace-callout is-error" role="alert">Repositories: {data.repositories.error}</p>
      {/if}
      <label>Title<input bind:value={createTitle} required /></label>
      <label>Body<textarea bind:value={createBody} rows="8"></textarea></label>
      <div class="ticket-target-list">
        <strong>Repository resources (optional)</strong>
        {#each createTargets as target, index}
          {@const selectedRepository = repositories.find((repository) => repository.repository_key === target.repository_key)}
          <fieldset class="ticket-target-row">
            <legend>Target {index + 1}</legend>
            <label>Repository
              <select bind:value={target.repository_key} required>
                <option value="">Choose repository</option>
                {#each repositories as repository}
                  <option value={repository.repository_key}>{repository.repository_key}</option>
                {/each}
              </select>
            </label>
            <label>Ref selector<input bind:value={target.ref_selector} placeholder={selectedRepository?.default_selector ?? "branch, tag, or commit"} /></label>
            <label>Access
              <select bind:value={target.access}>
                <option value="read_write">Read and write</option>
                <option value="read_only">Read only</option>
              </select>
            </label>
            <button class="workspace-secondary-button" type="button" onclick={() => removeCreateTarget(index)}>Remove target</button>
          </fieldset>
        {/each}
        <button class="workspace-secondary-button" type="button" onclick={addCreateTarget}>Add target</button>
      </div>
      {#if !createTargetsSavable}
        <p class="workspace-empty-copy">Each optional resource row must select a different registered repository. Empty or read-only resources are allowed.</p>
      {/if}
      <button class="workspace-primary-button" type="submit" disabled={createBusy || !createTitle.trim() || !createTargetsSavable}>
        {createBusy ? "Creating…" : "Create Ticket"}
      </button>
    </form>
  {/if}

  {#if orchestrator.error}
    <p class="workspace-callout is-error">
      Orchestrator status: {orchestrator.error}
    </p>
  {/if}

  <form class="ticket-query-form" onsubmit={applyFilters} aria-label="Ticket filters">
    <label>Search<input name="q" value={data.filters.query} type="search" /></label>
    <label>Case<select name="case" value={data.filters.caseState}><option value="all">Open and closed</option><option value="open">Open</option><option value="closed">Closed</option></select></label>
    <label>Assigned principal<select name="principal" bind:value={principal}><option value="all">Any</option><option value="orchestrator">Orchestrator</option><option value="user">User</option><option value="worker">Worker</option></select></label>
    {#if principal === "user"}<label>Account ID<input name="account" value={data.filters.accountId} required /></label>{/if}
    {#if principal === "worker"}<label>Runtime ID<input name="runtime" value={data.filters.runtimeId} required /></label><label>Worker ID<input name="worker" value={data.filters.workerId} required /></label>{/if}
    <fieldset><legend>Classification tags (all selected)</legend>
      {#each data.tags.data ?? [] as tag (tag.id)}
        {#if !tag.deleted}<label class="ticket-tag-filter"><input type="checkbox" name="tag" value={tag.id} checked={data.filters.tagIds.includes(tag.id)} />{tag.name}</label>{/if}
      {:else}<span class="workspace-empty-copy">No tags</span>{/each}
      {#if data.tags.error}<p class="workspace-callout is-error" role="alert">Tags: {data.tags.error}</p>{/if}
    </fieldset>
    <details class="ticket-reason-filter" open={data.filters.closureReason !== "all"}><summary>Close reason filter</summary>
      <label>Current close reason<select name="reason" bind:value={closureChoice}><option value="all">Any reason</option><option value="completed">Completed</option><option value="not_planned">Not planned</option><option value="duplicate">Duplicate</option><option value="unknown">Unknown</option><option value="other">Other (exact text)</option></select></label>
      {#if closureChoice === "other"}<label>Close reason text<input name="reason_text" value={data.filters.closureText} required /></label>{/if}
      <p class="workspace-empty-copy">Matches current closed cases. Search also includes past close reasons.</p>
    </details>
    <button class="workspace-secondary-button" disabled={filtersBusy}>{filtersBusy ? "Searching…" : "Apply filters"}</button>
    <a href={`/w/${encodeURIComponent(data.workspaceId)}/orchestrator`}>Orchestrator handling</a>
  </form>

  <section class="ticket-kanban" aria-label="Ticket cases">
    {#each lanes as lane (lane.id)}
      {@const pagination = laneState[lane.id]}
      <section class="ticket-lane" data-state={lane.id}>
        <header class="ticket-lane-header">
          <div>
            <span class="ticket-state-dot"></span>
            <h2>{lane.label}</h2>
          </div>
          <span class="ticket-lane-count">{lane.tickets.length} loaded</span>
        </header>
        <div
          class="ticket-lane-cards"
          data-lane-id={lane.id}
        >
          {#each lane.tickets as ticket (ticket.id)}
            {@const assignment = currentOrchestratorAssignment(ticket)}
            <a
              class="ticket-card"
              href={ticketHref(data.workspaceId, ticket)}
            >
              <span class="ticket-card-id">{ticket.resource_key}</span>
              <strong>{ticket.title}</strong>
              <div class="ticket-card-meta">
                <span>Case: {ticket.state} · {ticket.priority}</span>
                <time>{prettyDate(ticket.updated_at)}</time>
              </div>
              {#if ticket.current_closure}<span class="ticket-closure-summary">Close reason: {closureReasonLabel(ticket.current_closure.reason)}</span>{/if}
              <span class="ticket-classification">Tags: {ticket.tags.length ? ticket.tags.map((tag) => tag.name).join(", ") : "None"}</span>
              <div class="ticket-handling-summary">
                <span>Orchestrator handling: {assignment ? handlingLabel(assignment.tracking?.status) : "Not assigned"}{#if assignment}{" "}<small>({assignment.tracking ? "self-reported" : "not reported"})</small>{/if}</span>
                {#if assignment}<span>Assigned principal: {assignmentPrincipalLabel(assignment.principal)}</span>{/if}
                <span>{handlingSummary(assignment)}</span>
              </div>
            </a>
          {:else}
            {#if !pagination?.error}<div class="ticket-lane-empty">No tickets</div>{/if}
          {/each}
          {#if pagination?.loading}
            <p class="ticket-lane-page-state" aria-live="polite">Loading…</p>
          {:else if pagination?.error}
            <div class="ticket-lane-page-state ticket-lane-page-error" role="alert">
              <span>{pagination.error}</span>
              <button type="button" onclick={() => loadMore(lane.id)}>Retry</button>
            </div>
          {:else if pagination?.page?.has_more}
            <button class="workspace-secondary-button" type="button" onclick={() => loadMore(lane.id)}>Load more {lane.label.toLowerCase()} tickets</button>
          {:else if pagination?.page && !pagination.page.has_more && lane.tickets.length > 0}
            <p class="ticket-lane-page-state">End of lane</p>
          {/if}
        </div>
      </section>
    {/each}
  </section>
</div>

<style>
  .ticket-query-form { display: flex; flex-wrap: wrap; align-items: end; gap: var(--space-3); margin-block: var(--space-4); }
  .ticket-query-form > label { display: grid; gap: var(--space-1); min-width: 0; flex: 1 1 10rem; }
  .ticket-query-form input:not([type="checkbox"]), .ticket-query-form select { min-width: 0; width: 100%; padding: var(--space-2); color: var(--text); background: var(--bg-raised); border: 1px solid var(--line); border-radius: var(--radius-soft); font: inherit; }
  .ticket-query-form fieldset { display: flex; flex-wrap: wrap; gap: var(--space-3); border: 0; padding: 0; flex: 1 1 100%; min-width: 0; }
  .ticket-tag-filter { display: flex; align-items: center; gap: var(--space-1); overflow-wrap: anywhere; }
  .ticket-reason-filter { flex: 1 1 100%; }
  .ticket-reason-filter label { display: grid; gap: var(--space-1); max-width: 28rem; margin-block: var(--space-2); }
  .ticket-reason-filter summary { cursor: pointer; }
  .ticket-query-form legend { margin-bottom: var(--space-2); }
  .ticket-kanban { grid-template-columns: repeat(auto-fit, minmax(min(100%, 20rem), 1fr)); align-items: start; overflow: visible; }
  .ticket-lane { height: auto; min-height: 0; max-height: none; }
  .ticket-lane-cards { max-height: none; overflow: visible; }
  .ticket-card { overflow-wrap: anywhere; }
  .ticket-handling-summary { display: grid; gap: var(--space-1); border-top: 1px solid var(--line); padding-top: var(--space-2); color: var(--text-muted); }
  .ticket-classification { color: var(--text-muted); }
  .ticket-closure-summary { font-weight: 600; }
</style>
