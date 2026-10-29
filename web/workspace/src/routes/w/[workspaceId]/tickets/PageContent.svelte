<script lang="ts">
  // 検索条件とページ順序はBackendのqueryに任せ、取得済みの一部を並べ替えない。
  // 案件状態と担当の自己申告は、同じ行でも別の情報として表示する。
  import { goto } from "$app/navigation";
  import { untrack } from "svelte";
  import type { ApiResult } from "#lib/workspace/api/http.ts";
  import {
    loadJson,
    workspaceApiJsonWithBody,
    workspaceApiPath,
  } from "#lib/workspace/api/http.ts";
  import { loadWorkspaceRepositoryList } from "#lib/workspace/api/repositories.ts";
  import type { RepositoryListResponse } from "#lib/generated/repository-api.ts";
  import {
    TICKET_BROWSER_API_LOAD_POLICY,
    TICKET_BROWSER_API_MAX_RESPONSE_BYTES,
    parseTicketBrowseQueryResponse,
    parseTicketRecordRef,
  } from "#lib/workspace/api/ticket-browser.ts";
  import type {
    NewTicket,
    TicketBrowseQueryItem as TicketSummary,
    TicketTarget,
    TicketTargetAccess,
  } from "#lib/generated/ticket-api.ts";
  import { ticketHref } from "#lib/workspace/resource-links.ts";
  import {
    currentOrchestratorAssignment, assignmentPrincipalLabel, handlingLabel, closureReasonLabel,
  } from "#lib/workspace/tickets/ticket-panel.ts";
  import type { PageData as RouteData } from "./$types";
  import type { ResolvedPageData } from "#lib/workspace/navigation/deferred.ts";

  type PageData = ResolvedPageData<RouteData>;

  type EditableTicketTarget = {
    repository_key: string;
    ref_selector: string;
    access: TicketTargetAccess;
  };

  let { data }: { data: PageData } = $props();
  let response = $state(untrack(() => data.tickets));
  let searchText = $state(untrack(() => data.search));
  let searchBusy = $state(false);
  let pageBusy = $state(false);
  let navigationError = $state<string | null>(null);
  let routeGeneration = 0;
  $effect(() => {
    const currentData = data;
    const generation = ++routeGeneration;
    untrack(() => {
      response = currentData.tickets;
      searchText = currentData.search;
      searchBusy = false;
      pageBusy = false;
      navigationError = null;
      repositoryResponse = { data: null, error: null };
      repositoriesLoading = false;
      creatingTicket = false;
      createBusy = false;
      createError = null;
      createTitle = "";
      createBody = "";
      createTargets = [];
    });
    return () => { ++routeGeneration; };
  });
  let creatingTicket = $state(false);
  let createBusy = $state(false);
  let createError = $state<string | null>(null);
  let createTitle = $state("");
  let createBody = $state("");
  let createTargets = $state<EditableTicketTarget[]>([]);
  let repositoryResponse = $state<ApiResult<RepositoryListResponse>>({ data: null, error: null });
  let repositoriesLoading = $state(false);
  const repositories = $derived(repositoryResponse.data?.items ?? []);
  const createTargetsSavable = $derived.by(() => {
    if (repositoriesLoading || !repositoryResponse.data) return false;
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
  function isCurrent(observedData: PageData, generation: number): boolean {
    return data === observedData && routeGeneration === generation;
  }

  async function loadRepositories(): Promise<void> {
    if (repositoriesLoading) return;
    const observedData = data;
    const generation = routeGeneration;
    repositoriesLoading = true;
    repositoryResponse = { data: null, error: null };
    const result = await loadWorkspaceRepositoryList(fetch, observedData.workspaceId);
    if (!isCurrent(observedData, generation)) return;
    repositoryResponse = result;
    repositoriesLoading = false;
  }

  function toggleCreate(): void {
    creatingTicket = !creatingTicket;
    if (creatingTicket && !repositoryResponse.data) void loadRepositories();
  }

  async function searchTickets(event?: SubmitEvent) {
    event?.preventDefault();
    if (searchBusy) return;
    const observedData = data;
    const generation = routeGeneration;
    searchBusy = true;
    navigationError = null;
    try {
      const query = new URLSearchParams({ q: searchText.trim() });
      await goto(`/w/${encodeURIComponent(data.workspaceId)}/tickets?${query}`, {
        reset: false, refreshAll: true,
      });
    } catch (error) {
      if (isCurrent(observedData, generation)) {
        navigationError = error instanceof Error ? error.message : String(error);
      }
    } finally {
      if (isCurrent(observedData, generation)) searchBusy = false;
    }
  }

  function mergeTickets(
    current: TicketSummary[],
    incoming: TicketSummary[],
  ): TicketSummary[] {
    const byId = new Map(current.map((ticket) => [ticket.id, ticket]));
    for (const ticket of incoming) byId.set(ticket.id, ticket);
    return [...byId.values()];
  }

  async function loadMore(): Promise<void> {
    if (pageBusy || searchBusy || !data.queryRequest || (response.data && !response.data.page.has_more)) return;
    const observedData = data;
    const generation = routeGeneration;
    const previous = response.data;
    pageBusy = true;
    response = { data: previous, error: null };
    const result = await loadJson(
      fetch,
      workspaceApiPath(observedData.workspaceId, "/tickets/browse"),
      {
        method: "POST", headers: { "content-type": "application/json" },
        body: JSON.stringify({ ...observedData.queryRequest, cursor: previous?.page.next_cursor ?? null }),
      },
      parseTicketBrowseQueryResponse,
      TICKET_BROWSER_API_LOAD_POLICY,
    );
    // 旧query/Workspaceの遅延応答で、現在の一覧やページ状態を上書きしない。
    if (!isCurrent(observedData, generation)) return;
    response = result.data
      ? { data: { ...result.data, items: mergeTickets(previous?.items ?? [], result.data.items) }, error: null }
      : { data: previous, error: result.error };
    pageBusy = false;
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
    const observedData = data;
    const generation = routeGeneration;
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
        workspaceApiPath(observedData.workspaceId, "/tickets"),
        { method: "POST", body: JSON.stringify(request) },
        parseTicketRecordRef,
        TICKET_BROWSER_API_MAX_RESPONSE_BYTES,
      );
      if (!isCurrent(observedData, generation)) return;
      const reference = created.resource_key ?? created.id;
      await goto(
        `/w/${encodeURIComponent(data.workspaceId)}/tickets/${encodeURIComponent(reference)}`,
      );
    } catch (error) {
      if (isCurrent(observedData, generation)) {
        createError = error instanceof Error ? error.message : String(error);
      }
    } finally {
      if (isCurrent(observedData, generation)) createBusy = false;
    }
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
        onclick={toggleCreate}
      >{creatingTicket ? "Cancel" : "Add Ticket"}</button>

    </div>
  </header>

  {#if creatingTicket}
    <form class="ticket-editor ticket-create-form" onsubmit={createTicket}>
      <h2>New Ticket</h2>
      {#if createError}<p class="workspace-callout is-error" role="alert">{createError}</p>{/if}
      {#if repositoriesLoading}
        <p role="status">Loading repositories…</p>
      {:else if repositoryResponse.error}
        <p class="workspace-callout is-error" role="alert">Repositories: {repositoryResponse.error}</p>
        <button class="workspace-secondary-button" type="button" onclick={loadRepositories}>Retry repositories</button>
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


  <form class="ticket-query-form" onsubmit={searchTickets} aria-label="Search tickets">
    <input name="q" type="search" aria-label="Search tickets" bind:value={searchText}
      placeholder="Search tickets · state:open" spellcheck="false" />
    <button class="workspace-secondary-button" type="submit" disabled={searchBusy}>
      {searchBusy ? "Searching…" : "Search"}
    </button>
  </form>
  <div class="ticket-search-context">
    <details class="ticket-search-help">
      <summary>Search syntax</summary>
      <p>Combine text with <code>state:open</code> or <code>state:closed</code>. Omit state to search both.</p>
      <p><code>tag:"UI bug"</code> · <code>assignee:subject:SUBJECT_ID</code> · <code>reason:completed</code></p>
      <p>Other principals: <code>assignee:user/account-id</code> or <code>assignee:worker/runtime-id/worker-id</code>.
        Custom close reason: <code>reason:"Superseded plan"</code>.</p>
    </details>
    <a href={'/w/' + encodeURIComponent(data.workspaceId) + '/orchestrator'}>Subject handling</a>
  </div>
  {#if navigationError}<p class="workspace-callout is-error" role="alert">{navigationError}</p>{/if}

  <section class="ticket-list" aria-label="Tickets" aria-busy={searchBusy || pageBusy}>
    {#if response.data}
      <p class="ticket-list-count" aria-live="polite">{response.data.items.length} tickets loaded</p>
    {/if}
    <ul class="ticket-list-rows">
      {#each response.data?.items ?? [] as ticket (ticket.id)}
        {@const assignment = currentOrchestratorAssignment(ticket)}
        <li>
          <a class="ticket-list-row" data-state={ticket.state} href={ticketHref(data.workspaceId, ticket)}>
            <span class="ticket-list-state">{ticket.state === "open" ? "Open" : "Closed"}</span>
            <div class="ticket-list-content">
              <strong class="ticket-list-title">{ticket.title}</strong>
              <div class="ticket-list-meta">
                <span class="ticket-list-key">{ticket.resource_key}</span>
                <time datetime={ticket.updated_at ?? undefined}>{prettyDate(ticket.updated_at)}</time>
                {#each ticket.tags as tag (tag.id)}<span class="ticket-list-tag">{tag.name}</span>{/each}
                {#if ticket.current_closure}<span>Close reason: {closureReasonLabel(ticket.current_closure.reason)}</span>{/if}
                {#if assignment}<span title={assignmentPrincipalLabel(assignment.principal)}>Handling: {handlingLabel(assignment.tracking?.status)} ({assignment.tracking ? "self-reported" : "not reported"})</span>{/if}
              </div>
            </div>
          </a>
        </li>
      {/each}
    </ul>
    {#if response.error}
      <div class="ticket-list-error" role="alert">
        <span>{response.error}</span>
        <button class="workspace-secondary-button" type="button" disabled={pageBusy || searchBusy}
          onclick={() => data.queryRequest ? loadMore() : searchTickets()}>Retry</button>
      </div>
    {:else if !response.data?.items.length && !searchBusy}
      <p class="ticket-list-empty">No tickets match this query.</p>
    {/if}
    {#if pageBusy}
      <p class="ticket-list-page-state" aria-live="polite">Loading…</p>
    {:else if response.data?.page.has_more && !response.error}
      <button class="workspace-secondary-button ticket-load-more" type="button" disabled={searchBusy} onclick={loadMore}>Load more tickets</button>
    {/if}
  </section>
</div>

<style>
  .ticket-query-form { display: flex; align-items: stretch; gap: var(--space-2); margin-top: var(--space-4); }
  .ticket-query-form input { flex: 1; min-width: 0; padding: var(--space-2) var(--space-3); color: var(--text); background: var(--bg); border: 1px solid var(--line); border-radius: var(--radius-soft); font: inherit; }
  .ticket-query-form input:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .ticket-search-context { display: flex; align-items: start; justify-content: space-between; flex-wrap: wrap; gap: var(--space-2); margin-block: var(--space-2) var(--space-4); color: var(--text-muted); font-size: var(--font-size-compact); }
  .ticket-search-context a { color: inherit; }
  .ticket-search-help { min-width: 0; max-width: 100%; }
  .ticket-search-help summary { cursor: pointer; width: fit-content; }
  .ticket-search-help p { margin-block: var(--space-2); overflow-wrap: anywhere; }
  .ticket-search-help code { font-size: inherit; }
  .ticket-list { min-width: 0; }
  .ticket-list-count { margin: 0; padding-block: var(--space-2); color: var(--text-muted); font-size: var(--font-size-compact); }
  .ticket-list-rows { list-style: none; margin: 0; padding: 0; border-top: 1px solid var(--line); }
  .ticket-list-row { display: grid; grid-template-columns: 4.5rem minmax(0, 1fr); align-items: start; gap: var(--space-2); padding-block: var(--space-3); border-bottom: 1px solid var(--line); color: var(--text); text-decoration: none; }
  .ticket-list-row:hover { background: var(--interactive-hover); }
  .ticket-list-row:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
  .ticket-list-state { padding-top: 2px; font-size: var(--font-size-compact); color: var(--text-muted); }
  .ticket-list-row[data-state="open"] .ticket-list-state { color: var(--success); }
  .ticket-list-content { min-width: 0; }
  .ticket-list-title { display: block; font-size: var(--font-size-body); line-height: var(--line-height-body); color: var(--text-strong); overflow-wrap: anywhere; }
  .ticket-list-meta { display: flex; flex-wrap: wrap; gap: var(--space-1) var(--space-3); margin-top: var(--space-1); font-size: var(--font-size-compact); color: var(--text-muted); overflow-wrap: anywhere; }
  .ticket-list-key { font-family: var(--font-mono); }
  .ticket-list-tag { color: var(--text); }
  .ticket-list-empty, .ticket-list-page-state { padding-block: var(--space-4); margin: 0; color: var(--text-muted); }
  .ticket-list-error { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-3); padding-block: var(--space-4); color: var(--danger); overflow-wrap: anywhere; }
  .ticket-load-more { margin-block: var(--space-4); }
  @media (max-width: 480px) { .ticket-list-row { grid-template-columns: 3.5rem minmax(0, 1fr); } }
</style>
