<script lang="ts">
  import { untrack } from "svelte";
  import RichMarkdown from "$lib/workspace/console/RichMarkdown.svelte";
  import {
    workspaceApiJsonWithBody,
    workspaceApiPath,
  } from "$lib/workspace/api/http";
  import {
    relationLabel,
    TICKET_STATES,
    ticketWorkerLaunchHref,
    type WorkspaceOrchestratorStatus,
  } from "$lib/workspace/tickets/ticket-panel";
  import type { ApiResult } from "$lib/workspace/api/http";
  import type {
    RepositoryListResponse,
    TicketDetail,
  } from "$lib/workspace/sidebar/types";

  type MergeRequestThreadEvent =
    | {
      kind: "review_requested";
      event_id: string;
      sequence: number;
      subject_ref: string;
      requested_by: { runtime_id: string; worker_id: string };
      reviewer: { runtime_id: string; worker_id: string };
    }
    | {
      kind: "review";
      event_id: string;
      sequence: number;
      request_event_id: string;
      subject_ref: string;
      decision: "approve" | "request_changes";
      body: string;
      reviewer: { runtime_id: string; worker_id: string };
    }
    | { kind: "review_revoked"; sequence: number; review_event_id: string; reason: string }
    | { kind: "review_cancelled"; sequence: number; request_event_id: string; reason: string }
    | {
      kind: "comment";
      sequence: number;
      body: string;
      author: { runtime_id: string; worker_id: string };
    }
    | {
      kind: "merge";
      sequence: number;
      approval_event_id: string;
      approved_source_ref: string;
      target_ref_after: string;
      strategy: "fast_forward" | "merge";
      resolution: "none" | "clean" | "conflicts_resolved";
      merged_by: { runtime_id: string; worker_id: string };
    };

  type RefProjection = { status: "known" | "unknown" | "requires_repair"; ref?: string };
  type MergeRequestDetail = {
    state: "open" | "closed" | "merged";
    selector_from: string | null;
    selector_to: string;
    source: RefProjection;
    target: RefProjection;
    thread: MergeRequestThreadEvent[];
  };

  const MUTABLE_TICKET_STATES = TICKET_STATES.filter((state) => state !== "done");

  const { data } = $props<{
    data: {
      workspaceId: string;
      ticketId: string;
      ticket: ApiResult<TicketDetail>;
      repositories: ApiResult<RepositoryListResponse>;
      orchestrator: ApiResult<WorkspaceOrchestratorStatus>;
      mergeRequest: ApiResult<MergeRequestDetail | null>;
    };
  }>();

  const initialData = untrack(() => data);
  const loadedTicket = initialData.ticket.data;
  if (!loadedTicket) throw new Error(initialData.ticket.error ?? "ticket load failed");
  const loadedRepositories = initialData.repositories.data;
  const orchestratorOnline = initialData.orchestrator.data?.online ?? false;

  let ticket = $state<TicketDetail>(loadedTicket);
  let mergeRequest = $state<MergeRequestDetail | null>(initialData.mergeRequest.data ?? null);
  const currentReviewRequest = $derived(
    mergeRequest?.thread.findLast((event) => event.kind === "review_requested") ?? null,
  );
  const currentReview = $derived(
    mergeRequest?.source.status === "known"
      ? mergeRequest.thread.findLast(
        (event) => event.kind === "review" && event.subject_ref === mergeRequest?.source.ref,
      ) ?? null
      : null,
  );
  const mergeEvent = $derived(
    mergeRequest?.thread.findLast((event) => event.kind === "merge") ?? null,
  );
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
              <a href={`/w/${encodeURIComponent(data.workspaceId)}/tickets/${encodeURIComponent(blocker.blocking_ticket)}`}>
                <strong>Blocked by {blocker.blocking_ticket}</strong>
                <span>{relationLabel(blocker.relation_kind)} · {blocker.blocking_state}</span>
              </a>
            {/each}
          </div>
        {/if}
        <div class="ticket-relations-list">
          {#each ticket.relations.outgoing as relation}
            <a href={`/w/${encodeURIComponent(data.workspaceId)}/tickets/${encodeURIComponent(relation.target)}`}>
              <span>{relationLabel(relation.kind)}</span>
              <strong>{relation.target}</strong>
              {#if relation.note}<small>{relation.note}</small>{/if}
            </a>
          {/each}
          {#each ticket.relations.incoming as relation}
            <a href={`/w/${encodeURIComponent(data.workspaceId)}/tickets/${encodeURIComponent(relation.source_ticket)}`}>
              <span>{relationLabel(relation.inverse_kind)}</span>
              <strong>{relation.source_ticket}</strong>
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
        {#if orchestratorOnline}
          <p>The Orchestrator is online. Start a role-specific Worker with the Ticket target below.</p>
          <div class="ticket-role-actions">
            <a class="workspace-primary-button" href={ticketWorkerLaunchHref(data.workspaceId, ticket, "coder")}>Coder</a>
          </div>
        {:else}
          <p class="workspace-callout">Start the Workspace Orchestrator from the Ticket panel before launching Ticket Workers.</p>
          <div class="ticket-role-actions">
            <button class="workspace-primary-button" type="button" disabled>Coder</button>
            <button class="workspace-secondary-button" type="button" disabled>Reviewer</button>
          </div>
        {/if}
      </section>

      <section class="ticket-control-card">
        <header><h2>Repository target</h2></header>
        <form class="ticket-control-form" onsubmit={saveTarget}>
          <label>Repository
            <select bind:value={repositoryId}>
              <option value="">Not assigned</option>
              {#each loadedRepositories?.items ?? [] as repository}
                <option value={repository.id}>{repository.display_name}</option>
              {/each}
            </select>
          </label>
          <label>Ref selector<input bind:value={refSelector} placeholder="branch, tag, or revision" /></label>
          <button class="workspace-secondary-button" type="submit" disabled={busy === "target"}>
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
        {#if ticket.state === "ready"}
          <button class="workspace-primary-button ticket-queue-button" type="button" disabled={busy === "queue" || !orchestratorOnline} onclick={() => mutate("queue", "/queue", {})}>
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
        {#if data.mergeRequest.error}
          <p class="workspace-callout is-error">{data.mergeRequest.error}</p>
        {:else if mergeRequest}
          <p><strong>{mergeRequest.state}</strong></p>
          <p>From <code>{mergeRequest.selector_from ?? "requires repair"}</code> · {mergeRequest.source.status}{mergeRequest.source.ref ? ` @ ${mergeRequest.source.ref}` : ""}</p>
          <p>To <code>{mergeRequest.selector_to}</code> · {mergeRequest.target.status}{mergeRequest.target.ref ? ` @ ${mergeRequest.target.ref}` : ""}</p>
          {#if currentReviewRequest?.kind === "review_requested"}
            <p>Review requested for <code>{currentReviewRequest.subject_ref}</code></p>
          {/if}
          {#if currentReview?.kind === "review"}
            <p><strong>{currentReview.decision}</strong> by <code>{currentReview.reviewer.runtime_id}/{currentReview.reviewer.worker_id}</code></p>
            {#if currentReview.body}<RichMarkdown text={currentReview.body} />{/if}
          {/if}
          {#if mergeEvent?.kind === "merge"}
            <p>Final merge · {mergeEvent.strategy} / {mergeEvent.resolution}</p>
            <p>Target ref <code>{mergeEvent.target_ref_after}</code></p>
            <p>Completed by <code>{mergeEvent.merged_by.runtime_id}/{mergeEvent.merged_by.worker_id}</code></p>
          {/if}
          <h4>Thread</h4>
          {#each mergeRequest.thread as event (event.sequence)}
            <p>
              <code>#{event.sequence}</code> · {event.kind}
              {#if event.kind === "review_requested"}
                · <code>{event.subject_ref}</code> · {event.requested_by.runtime_id}/{event.requested_by.worker_id}
              {:else if event.kind === "review"}
                · <code>{event.subject_ref}</code> · {event.reviewer.runtime_id}/{event.reviewer.worker_id}
              {:else if event.kind === "comment"}
                · {event.author.runtime_id}/{event.author.worker_id} · {event.body}
              {:else if event.kind === "review_cancelled" || event.kind === "review_revoked"}
                · {event.reason}
              {:else if event.kind === "merge"}
                · approval <code>{event.approval_event_id}</code>
              {/if}
            </p>
          {/each}
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
