<script lang="ts">
  import { goto } from "$app/navigation";
  import { untrack } from "svelte";
  import type { ApiResult } from "$lib/workspace/api/http";
  import {
    loadJson,
    workspaceApiJsonWithBody,
    workspaceApiPath,
  } from "$lib/workspace/api/http";
  import { parseBrowserWorkspaceOrchestratorResponse } from "$lib/workspace/api/workers";
  import {
    TICKET_BROWSER_API_LOAD_POLICY,
    TICKET_BROWSER_API_MAX_RESPONSE_BYTES,
    parseTicketListResponse,
    parseTicketRecordRef,
  } from "$lib/workspace/api/ticket-browser";
  import type {
    NewTicket,
    QueryPage,
    TicketListItemSummary as TicketSummary,
    TicketTarget,
    TicketTargetAccess,
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

  type EditableTicketTarget = {
    repository_key: string;
    ref_selector: string;
    access: TicketTargetAccess;
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
      const result = await loadJson(
        fetch,
        `/api/w/${encodeURIComponent(data.workspaceId)}/tickets?${search}`,
        undefined,
        parseTicketListResponse,
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

  function handleLaneScroll(event: Event, laneId: string): void {
    const container = event.currentTarget as HTMLElement;
    const remaining =
      container.scrollHeight - container.scrollTop - container.clientHeight;
    if (remaining <= 96) void loadMore(laneId);
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
      workflow_state: "planning",
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
      <p class="workspace-eyebrow">Delivery</p>
      <h1>Tickets</h1>
      <p class="workspace-page-lede">
        Plan, route, review, and close work without leaving the workspace.
      </p>
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
        <strong>Repository targets</strong>
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
            <label>Ref selector<input bind:value={target.ref_selector} placeholder={selectedRepository?.default_selector ?? "branch, tag, or revision"} /></label>
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
        <p class="workspace-empty-copy">Every target row must select a different repository. The exact one read-write target and selector requirements are enforced when the Ticket is marked ready.</p>
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
