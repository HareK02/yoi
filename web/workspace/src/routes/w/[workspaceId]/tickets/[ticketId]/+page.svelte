<script lang="ts">
  import { untrack } from "svelte";
  import RichMarkdown from "$lib/workspace/console/RichMarkdown.svelte";
  import {
    workspaceApiJsonWithBody,
    workspaceApiPath,
  } from "$lib/workspace/api/http";
  import { mergeRequestPagePath } from "$lib/workspace/api/merge-requests";
  import {
    relationLabel,
    TICKET_STATES,
    ticketWorkerLaunchHref,
    type WorkspaceOrchestratorStatus,
  } from "$lib/workspace/tickets/ticket-panel";
  import type { ApiResult } from "$lib/workspace/api/http";
  import type {
    RepositoryListResponse,
    RepositorySummary,
    TicketDetail,
  } from "$lib/workspace/sidebar/types";

  const MUTABLE_TICKET_STATES = TICKET_STATES.filter((state) =>
    state !== "done" && state !== "ready" && state !== "queued"
  );

  const { data } = $props<{
    data: {
      workspaceId: string;
      ticketId: string;
      ticket: ApiResult<TicketDetail>;
      repositories: ApiResult<RepositoryListResponse>;
      orchestrator: ApiResult<WorkspaceOrchestratorStatus>;
    };
  }>();

  const initialData = untrack(() => data);
  const loadedTicket = initialData.ticket.data;
  if (!loadedTicket) throw new Error(initialData.ticket.error ?? "ticket load failed");
  const loadedRepositories = initialData.repositories.data;
  const orchestratorOnline = initialData.orchestrator.data?.online ?? false;

  let ticket = $state<TicketDetail>(loadedTicket);
  const mergeRequest = $derived(ticket.merge_request);
  let editing = $state(false);
  let editTitle = $state(loadedTicket.title);
  let editBody = $state(loadedTicket.body);
  let repositoryId = $state(loadedTicket.repository_id ?? "");
  let refSelector = $state(loadedTicket.ref_selector ?? "");
  let nextState = $state(loadedTicket.state);
  let transitionReason = $state("");
  let threadRole = $state("comment");
  let threadBody = $state("");
  let resolution = $state("");
  let busy = $state<string | null>(null);
  let errorMessage = $state<string | null>(null);
  let readyOperationKey = $state<string | null>(null);
  const selectedRepository = $derived(
    (loadedRepositories?.items ?? []).find((repository: RepositorySummary) => repository.id === repositoryId) ?? null,
  );
  const effectiveRefSelector = $derived(refSelector.trim() || selectedRepository?.default_ref || "");
  const targetCandidateValid = $derived(
    ticket.state === "planning" &&
      selectedRepository !== null &&
      (selectedRepository.diagnostics ?? []).length === 0 &&
      effectiveRefSelector.length > 0,
  );
  const persistedTargetValid = $derived(
    ticket.repository_id !== null &&
      ticket.ref_selector !== null &&
      (loadedRepositories?.items ?? []).some((repository: RepositorySummary) =>
        repository.id === ticket.repository_id && (repository.diagnostics ?? []).length === 0
      ),
  );
  const implementationStartEligible = $derived(
    persistedTargetValid && ticket.state !== "planning" && ticket.state !== "closed",
  );

  const ticketPath = $derived(
    workspaceApiPath(
      data.workspaceId,
      `/tickets/${encodeURIComponent(data.ticketId)}`,
    ),
  );

  function applyTicket(updatedTicket: TicketDetail): void {
    ticket = updatedTicket;
    editTitle = ticket.title;
    editBody = ticket.body;
    repositoryId = ticket.repository_id ?? "";
    refSelector = ticket.ref_selector ?? "";
    nextState = ticket.state;
  }

  async function mutate(
    action: string,
    suffix: string,
    body?: Record<string, unknown>,
    method = "POST",
  ): Promise<boolean> {
    if (busy) return false;
    busy = action;
    errorMessage = null;
    try {
      const path = `${ticketPath}${suffix}`;
      const response = await workspaceApiJsonWithBody<TicketDetail>(path, {
        method,
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      });
      applyTicket(response);
      return true;
    } catch (error) {
      errorMessage = error instanceof Error ? error.message : String(error);
      return false;
    } finally {
      busy = null;
    }
  }

  async function saveEdit(event: SubmitEvent) {
    event.preventDefault();
    if (
      await mutate("edit", "", {
        title: editTitle.trim(),
        body: editBody,
      }, "PATCH")
    ) editing = false;
  }

  async function saveTarget(event: SubmitEvent) {
    event.preventDefault();
    await mutate("target", "", {
      target: repositoryId
        ? {
          action: "set",
          repository_id: repositoryId,
          ref_selector: refSelector.trim() || null,
        }
        : { action: "clear" },
    }, "PATCH");
  }

  async function markReady() {
    if (!targetCandidateValid || busy) return;
    if (
      ticket.repository_id !== repositoryId ||
      (ticket.ref_selector ?? "") !== refSelector.trim()
    ) {
      const saved = await mutate("target", "", {
        target: {
          action: "set",
          repository_id: repositoryId,
          ref_selector: refSelector.trim() || null,
        },
      }, "PATCH");
      if (!saved) return;
    }
    readyOperationKey ??= crypto.randomUUID();
    if (
      await mutate("ready", "/ready", {
        operation_key: readyOperationKey,
        reason: transitionReason.trim() || null,
      })
    ) {
      readyOperationKey = null;
      transitionReason = "";
    }
  }

  async function transition(event: SubmitEvent) {
    event.preventDefault();
    if (
      await mutate("state", "/state", {
        state: nextState,
        reason: transitionReason.trim() || null,
      })
    ) transitionReason = "";
  }

  async function appendThread(event: SubmitEvent) {
    event.preventDefault();
    if (!threadBody.trim()) return;
    if (
      await mutate("thread", "/thread", {
        role: threadRole,
        body: threadBody.trim(),
      })
    ) threadBody = "";
  }

  async function closeTicket(event: SubmitEvent) {
    event.preventDefault();
    if (!resolution.trim()) return;
    if (
      await mutate("close", "/close", { resolution: resolution.trim() })
    ) resolution = "";
  }

  function eventTitle(kind: string): string {
    return relationLabel(kind);
  }

  function prettyDate(value?: string | null): string {
    if (!value) return "—";
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
  }
</script>

<svelte:head><title>{ticket.title} · Yoi</title></svelte:head>

<div class="workspace-page ticket-detail-page">
  <header class="ticket-detail-header">
    <div>
      <div class="ticket-detail-kicker">
        <span class="workspace-status-pill" data-status={ticket.state}>{ticket.state}</span>
      </div>
      <h1>{ticket.title}</h1>
      <p>Updated {prettyDate(ticket.updated_at)}</p>
    </div>
    <button class="workspace-secondary-button" type="button" onclick={() => editing = !editing}>
      {editing ? "Cancel edit" : "Edit ticket"}
    </button>
  </header>

  {#if errorMessage}
    <div class="workspace-callout is-error" role="alert">{errorMessage}</div>
  {/if}

  {#if editing}
    <form class="ticket-editor" onsubmit={saveEdit}>
      <label>Title<input bind:value={editTitle} required /></label>
      <label>Body<textarea bind:value={editBody} rows="12"></textarea></label>
      <button class="workspace-primary-button" type="submit" disabled={busy === "edit" || !editTitle.trim()}>
        {busy === "edit" ? "Saving…" : "Save changes"}
      </button>
    </form>
  {/if}

  <div class="ticket-detail-grid">
    <main class="ticket-detail-main">
      <section class="ticket-detail-section">
        <div class="ticket-section-heading"><h2>Intent</h2></div>
        {#if ticket.body}
          <RichMarkdown text={ticket.body} />
        {:else}
          <p class="workspace-empty-copy">No body has been recorded.</p>
        {/if}
      </section>

      <section class="ticket-detail-section">
        <div class="ticket-section-heading">
          <h2>Relations</h2>
          <span>{ticket.relations.outgoing.length + ticket.relations.incoming.length}</span>
        </div>
        {#if ticket.relations.blockers.length > 0}
          <div class="ticket-blocker-list">
            {#each ticket.relations.blockers as blocker}
              <a href={`/w/${encodeURIComponent(data.workspaceId)}/tickets/${encodeURIComponent(blocker.blocking_human_key ?? blocker.blocking_ticket)}`}>
                <strong>Blocked by {blocker.blocking_human_key ?? blocker.blocking_ticket}</strong>
                <span>{relationLabel(blocker.relation_kind)} · {blocker.blocking_state}</span>
              </a>
            {/each}
          </div>
        {/if}
        <div class="ticket-relations-list">
          {#each ticket.relations.outgoing as relation}
            <a href={`/w/${encodeURIComponent(data.workspaceId)}/tickets/${encodeURIComponent(relation.target_human_key ?? relation.target)}`}>
              <span>{relationLabel(relation.kind)}</span>
              <strong>{relation.target_human_key ?? relation.target}</strong>
              {#if relation.note}<small>{relation.note}</small>{/if}
            </a>
          {/each}
          {#each ticket.relations.incoming as relation}
            <a href={`/w/${encodeURIComponent(data.workspaceId)}/tickets/${encodeURIComponent(relation.source_human_key ?? relation.source_ticket)}`}>
              <span>{relationLabel(relation.inverse_kind)}</span>
              <strong>{relation.source_human_key ?? relation.source_ticket}</strong>
              {#if relation.note}<small>{relation.note}</small>{/if}
            </a>
          {/each}
          {#if ticket.relations.outgoing.length === 0 && ticket.relations.incoming.length === 0}
            <p class="workspace-empty-copy">No Ticket relations.</p>
          {/if}
        </div>
      </section>

      <section class="ticket-detail-section">
        <div class="ticket-section-heading">
          <h2>Timeline</h2><span>{ticket.event_count}</span>
        </div>
        <div class="ticket-timeline">
          {#each ticket.events as event (event.sequence)}
            <article>
              <div class="ticket-timeline-marker"></div>
              <div>
                <header>
                  <strong>{event.heading ?? eventTitle(event.kind)}</strong>
                  <time>{prettyDate(event.at)}</time>
                </header>
                {#if event.author}<p class="ticket-event-author">{event.author}</p>{/if}
                {#if event.from || event.to}<p>{event.from ?? "—"} → {event.to ?? "—"}</p>{/if}
                {#if event.reason}<p>{event.reason}</p>{/if}
                {#if event.body}<RichMarkdown text={event.body} />{/if}
              </div>
            </article>
          {:else}
            <p class="workspace-empty-copy">No timeline events.</p>
          {/each}
        </div>
      </section>
    </main>

    <aside class="ticket-control-rail">
      <section class="ticket-control-card ticket-worker-card">
        <header><h2>Start a Worker</h2><span>Ticket role</span></header>
        <p class="ticket-assignment-line">
          Assigned to <strong>{ticket.assignee ?? "Unassigned"}</strong>
        </p>
        {#if orchestratorOnline && implementationStartEligible}
          <p>The Orchestrator is online. Start a role-specific Worker with the validated Ticket target below.</p>
          <div class="ticket-role-actions">
            <a class="workspace-primary-button" href={ticketWorkerLaunchHref(data.workspaceId, ticket, "coder")}>Coder</a>
          </div>
        {:else}
          <p class="workspace-callout">
            {orchestratorOnline
              ? "Validate and persist the repository target before starting a Ticket Worker."
              : "Start the Workspace Orchestrator from the Ticket panel before launching Ticket Workers."}
          </p>
          <div class="ticket-role-actions">
            <button class="workspace-primary-button" type="button" disabled>Coder</button>
          </div>
        {/if}
      </section>

      <section class="ticket-control-card">
        <header><h2>Repository target</h2></header>
        <form class="ticket-control-form" onsubmit={saveTarget}>
          <label>Repository
            <select bind:value={repositoryId} disabled={ticket.state !== "planning"}>
              <option value="">Not assigned</option>
              {#each loadedRepositories?.items ?? [] as repository}
                <option value={repository.id}>{repository.display_name}</option>
              {/each}
            </select>
          </label>
          <label>Ref selector<input bind:value={refSelector} placeholder={selectedRepository?.default_ref ?? "branch, tag, or revision"} disabled={ticket.state !== "planning"} /></label>
          <button class="workspace-secondary-button" type="submit" disabled={busy === "target" || ticket.state !== "planning"}>
            {busy === "target" ? "Saving…" : "Save target"}
          </button>
        </form>
      </section>

      <section class="ticket-control-card">
        <header><h2>Workflow</h2></header>
        <form class="ticket-control-form" onsubmit={transition}>
          <label>State
            <select bind:value={nextState}>
              {#each MUTABLE_TICKET_STATES as state}<option value={state}>{state}</option>{/each}
            </select>
          </label>
          <label>Reason<input bind:value={transitionReason} placeholder="Optional decision context" /></label>
          <button class="workspace-secondary-button" type="submit" disabled={busy === "state" || nextState === ticket.state}>
            Apply state
          </button>
        </form>
        {#if ticket.state === "planning"}
          <button class="workspace-primary-button ticket-queue-button" type="button" disabled={busy !== null || !targetCandidateValid} onclick={markReady}>
            {busy === "ready" ? "Marking ready…" : "Mark ready"}
          </button>
          {#if !targetCandidateValid}
            <p class="workspace-empty-copy">Choose a healthy repository and an effective ref selector before marking ready.</p>
          {/if}
        {:else if ticket.state === "ready"}
          <button class="workspace-primary-button ticket-queue-button" type="button" disabled={busy === "queue" || !orchestratorOnline || !persistedTargetValid} onclick={() => mutate("queue", "/queue", {})}>
            {busy === "queue" ? "Queueing…" : orchestratorOnline ? "Queue ticket" : "Orchestrator offline"}
          </button>
        {/if}
      </section>

      <details class="ticket-control-card">
        <summary>Append timeline event</summary>
        <form class="ticket-control-form" onsubmit={appendThread}>
          <label>Role<select bind:value={threadRole}>
            <option value="comment">Comment</option>
            <option value="plan">Plan</option>
            <option value="decision">Decision</option>
            <option value="implementation_report">Implementation report</option>
          </select></label>
          <label>Body<textarea bind:value={threadBody} rows="5" required></textarea></label>
          <button class="workspace-secondary-button" type="submit" disabled={busy === "thread" || !threadBody.trim()}>Append event</button>
        </form>
      </details>

      <section class="ticket-control-card">
        <header><h2>Merge Request</h2></header>
        {#if mergeRequest}
          <p><strong>{mergeRequest.state}</strong> · review {mergeRequest.review_status}</p>
          <p>
            From <code>{mergeRequest.selector_from ?? "requires repair"}</code>
            to <code>{mergeRequest.selector_to}</code>
          </p>
          {#if mergeRequest.current_subject_ref}
            <p>Current source <code>{mergeRequest.current_subject_ref}</code></p>
          {/if}
          <a
            class="workspace-secondary-button"
            href={mergeRequestPagePath(data.workspaceId, mergeRequest.merge_request_id)}
          >Open Merge Request</a>
        {:else}
          <p class="workspace-empty-copy">The assigned Coder has not opened a Merge Request.</p>
        {/if}
      </section>

      {#if ticket.state !== "closed"}
        <details class="ticket-control-card ticket-close-card">
          <summary>Close ticket</summary>
          <form class="ticket-control-form" onsubmit={closeTicket}>
            <label>Resolution<textarea bind:value={resolution} rows="5" required></textarea></label>
            <button class="workspace-danger-button" type="submit" disabled={busy === "close" || !resolution.trim()}>Close ticket</button>
          </form>
        </details>
      {:else if ticket.resolution}
        <section class="ticket-control-card"><header><h2>Resolution</h2></header><RichMarkdown text={ticket.resolution} /></section>
      {/if}
    </aside>
  </div>
</div>
