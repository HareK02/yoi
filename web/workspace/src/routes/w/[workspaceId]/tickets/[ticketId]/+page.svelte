<script lang="ts">
  import { untrack } from "svelte";
  import RichMarkdown from "$lib/workspace/console/RichMarkdown.svelte";
  import {
    workspaceApiJson,
    workspaceApiJsonWithBody,
    workspaceApiPath,
  } from "$lib/workspace/api/http";
  import {
    TICKET_BROWSER_API_MAX_RESPONSE_BYTES,
    parseTicketDetail,
    parseTicketQueueOutcome,
    parseTicketRoleAssignmentMutationResponse,
  } from "$lib/workspace/api/ticket-browser";
  import { mergeRequestPagePath } from "$lib/workspace/api/merge-requests";
  import {
    relationLabel,
    TICKET_STATES,
    type WorkspaceOrchestratorStatus,
  } from "$lib/workspace/tickets/ticket-panel";
  import type { ApiResult } from "$lib/workspace/api/http";
  import type {
    TicketTarget,
    TicketTargetAccess,
  } from "$lib/generated/ticket-api";
  import type {
    RepositoryListResponse,
    RepositorySummary,
    TicketDetail,
  } from "$lib/workspace/sidebar/types";

  type EditableTicketTarget = {
    repository_key: string;
    ref_selector: string;
    access: TicketTargetAccess;
  };

  const MUTABLE_TICKET_STATES = TICKET_STATES.filter((state) =>
    state !== "done" && state !== "ready" && state !== "queued"
  );

  function editableTargets(targets: TicketTarget[]): EditableTicketTarget[] {
    return targets.map((target) => ({
      repository_key: target.repository_key,
      ref_selector: target.ref_selector ?? "",
      access: target.access,
    }));
  }

  function normalizedTargets(targets: EditableTicketTarget[]): TicketTarget[] {
    return targets.map((target) => ({
      repository_key: target.repository_key.trim(),
      ref_selector: target.ref_selector.trim() || null,
      access: target.access,
    }));
  }

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
  const loadedRepositories = $derived(data.repositories.data);

  let ticket = $state<TicketDetail>(loadedTicket);
  const mergeRequest = $derived(ticket.merge_request);
  let editing = $state(false);
  let editTitle = $state(loadedTicket.title);
  let editBody = $state(loadedTicket.body);
  let targetDrafts = $state<EditableTicketTarget[]>(
    editableTargets(loadedTicket.targets),
  );
  let nextState = $state(loadedTicket.state);
  let transitionReason = $state("");
  let threadRole = $state("comment");
  let threadBody = $state("");
  let resolution = $state("");
  let busy = $state<string | null>(null);
  let errorMessage = $state<string | null>(null);
  let queueMessage = $state<string | null>(null);
  let readyOperationKey = $state<string | null>(null);
  let manualRuntimeId = $state("");
  let manualWorkerId = $state("");
  let cancellationReason = $state("");
  let routeTicketSnapshot = `${initialData.ticketId}:${loadedTicket.item_revision}`;
  let routeGeneration = 0;
  const coderAssignment = $derived(
    ticket.assignments.find((assignment) => assignment.role === "coder") ?? null,
  );
  function repositoryFor(repositoryKey: string): RepositorySummary | null {
    return (loadedRepositories?.items ?? []).find((repository: RepositorySummary) =>
      repository.repository_key === repositoryKey
    ) ?? null;
  }

  function effectiveRefSelector(target: EditableTicketTarget): string {
    return target.ref_selector.trim() ||
      repositoryFor(target.repository_key)?.default_selector || "";
  }

  const targetCandidateValid = $derived.by(() => {
    if (ticket.state !== "planning" || targetDrafts.length === 0) return false;
    const repositoryKeys = new Set<string>();
    let readWriteCount = 0;
    for (const target of targetDrafts) {
      const repository = repositoryFor(target.repository_key);
      if (
        !target.repository_key || repository === null ||
        (repository.diagnostics ?? []).length > 0 ||
        !effectiveRefSelector(target) ||
        repositoryKeys.has(target.repository_key)
      ) return false;
      repositoryKeys.add(target.repository_key);
      if (target.access === "read_write") readWriteCount += 1;
    }
    return readWriteCount === 1;
  });
  const implementationStartEligible = $derived(
    ticket.action_eligibility.can_start_manual_coder,
  );

  const ticketPath = $derived(
    workspaceApiPath(
      data.workspaceId,
      `/tickets/${encodeURIComponent(data.ticketId)}`,
    ),
  );

  function applyTicket(updatedTicket: TicketDetail): void {
    ticket = updatedTicket;
    editTitle = updatedTicket.title;
    editBody = updatedTicket.body;
    targetDrafts = editableTargets(updatedTicket.targets);
    nextState = updatedTicket.state;
  }

  function resetTicketView(updatedTicket: TicketDetail): void {
    applyTicket(updatedTicket);
    editing = false;
    transitionReason = "";
    threadRole = "comment";
    threadBody = "";
    resolution = "";
    busy = null;
    errorMessage = null;
    queueMessage = null;
    readyOperationKey = null;
    manualRuntimeId = "";
    manualWorkerId = "";
    cancellationReason = "";
  }

  $effect(() => {
    const incomingTicketId = data.ticketId;
    const incomingTicket = data.ticket.data;
    if (!incomingTicket) return;
    const incomingSnapshot = `${incomingTicketId}:${incomingTicket.item_revision}`;

    untrack(() => {
      if (incomingSnapshot === routeTicketSnapshot) return;
      routeTicketSnapshot = incomingSnapshot;
      routeGeneration += 1;
      resetTicketView(incomingTicket);
    });
  });

  async function mutate(
    action: string,
    suffix: string,
    body?: Record<string, unknown>,
    method = "POST",
  ): Promise<boolean> {
    if (busy) return false;
    const generation = routeGeneration;
    const path = `${ticketPath}${suffix}`;
    busy = action;
    errorMessage = null;
    try {
      const response = await workspaceApiJsonWithBody(path, {
        method,
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      }, parseTicketDetail, TICKET_BROWSER_API_MAX_RESPONSE_BYTES);
      if (generation !== routeGeneration) return false;
      applyTicket(response);
      return true;
    } catch (error) {
      if (generation === routeGeneration) {
        errorMessage = error instanceof Error ? error.message : String(error);
      }
      return false;
    } finally {
      if (generation === routeGeneration) busy = null;
    }
  }

  async function queueTicket(): Promise<void> {
    if (busy) return;
    const generation = routeGeneration;
    const path = ticketPath;
    busy = "queue";
    errorMessage = null;
    queueMessage = null;
    try {
      const outcome = await workspaceApiJsonWithBody(
        `${path}/queue`,
        { method: "POST", body: JSON.stringify({}) },
        parseTicketQueueOutcome,
        TICKET_BROWSER_API_MAX_RESPONSE_BYTES,
      );
      if (generation !== routeGeneration) return;
      const updatedTicket = await workspaceApiJson(
        path,
        parseTicketDetail,
        TICKET_BROWSER_API_MAX_RESPONSE_BYTES,
      );
      if (generation !== routeGeneration) return;
      queueMessage = `Queued ${outcome.queued_tickets.length} Ticket(s): ${outcome.queued_tickets.join(", ")}`;
      applyTicket(updatedTicket);
    } catch (error) {
      if (generation === routeGeneration) {
        errorMessage = error instanceof Error ? error.message : String(error);
      }
    } finally {
      if (generation === routeGeneration) busy = null;
    }
  }

  async function mutateAssignment(
    action: string,
    role: "orchestrator" | "coder",
    principal: Record<string, string>,
  ): Promise<void> {
    if (busy) return;
    const generation = routeGeneration;
    const path = ticketPath;
    busy = action;
    errorMessage = null;
    try {
      await workspaceApiJsonWithBody(
        `${path}/assignments/${role}`,
        {
          method: "PUT",
          body: JSON.stringify({
            operation_id: crypto.randomUUID(),
            principal,
            expected_assignment_id: null,
          }),
        },
        parseTicketRoleAssignmentMutationResponse,
        TICKET_BROWSER_API_MAX_RESPONSE_BYTES,
      );
      if (generation !== routeGeneration) return;
      const updatedTicket = await workspaceApiJson(
        path,
        parseTicketDetail,
        TICKET_BROWSER_API_MAX_RESPONSE_BYTES,
      );
      if (generation !== routeGeneration) return;
      applyTicket(updatedTicket);
    } catch (error) {
      if (generation === routeGeneration) {
        errorMessage = error instanceof Error ? error.message : String(error);
      }
    } finally {
      if (generation === routeGeneration) busy = null;
    }
  }

  async function assignOrchestrator(): Promise<void> {
    await mutateAssignment("assign-orchestrator", "orchestrator", {
      kind: "workspace_agent",
      agent_key: "workspace-orchestrator",
    });
  }

  async function startManualCoder(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (!manualRuntimeId.trim() || !manualWorkerId.trim()) return;
    await mutateAssignment("start-manual", "coder", {
      kind: "worker",
      runtime_id: manualRuntimeId.trim(),
      worker_id: manualWorkerId.trim(),
    });
  }

  async function cancelImplementation(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (!coderAssignment || !cancellationReason.trim()) return;
    if (
      await mutate("cancel-implementation", "/implementation-cancellations", {
        operation_id: crypto.randomUUID(),
        assignment_id: coderAssignment.assignment_id,
        reason: cancellationReason.trim(),
      })
    ) cancellationReason = "";
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

  function addTarget(access: TicketTargetAccess = "read_only"): void {
    if (ticket.state !== "planning") return;
    targetDrafts.push({ repository_key: "", ref_selector: "", access });
  }

  function removeTarget(index: number): void {
    if (ticket.state !== "planning") return;
    targetDrafts.splice(index, 1);
  }

  function targetEditBody(): Record<string, unknown> {
    const targets = normalizedTargets(targetDrafts);
    return {
      target: targets.length > 0
        ? { action: "set", targets }
        : { action: "clear" },
    };
  }

  async function saveTarget(event: SubmitEvent) {
    event.preventDefault();
    await mutate("target", "", targetEditBody(), "PATCH");
  }

  async function markReady() {
    if (!targetCandidateValid || busy) return;
    const targets = normalizedTargets(targetDrafts);
    if (JSON.stringify(ticket.targets) !== JSON.stringify(targets)) {
      const saved = await mutate("target", "", targetEditBody(), "PATCH");
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
      await mutate("thread", "/events", {
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

  {#if queueMessage}
    <div class="workspace-callout" role="status">{queueMessage}</div>
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
              {#if blocker.blocking_resource_key}
                <a href={`/w/${encodeURIComponent(data.workspaceId)}/tickets/${encodeURIComponent(blocker.blocking_resource_key)}`}>
                  <strong>Blocked by {blocker.blocking_resource_key}</strong>
                  <span>{relationLabel(blocker.relation_kind)} · {blocker.blocking_state}</span>
                </a>
              {:else}
                <div>
                  <strong>Blocked by resource key unavailable</strong>
                  <span>{relationLabel(blocker.relation_kind)} · {blocker.blocking_state}</span>
                </div>
              {/if}
            {/each}
          </div>
        {/if}
        <div class="ticket-relations-list">
          {#each ticket.relations.outgoing as relation}
            {#if relation.target_resource_key}
              <a href={`/w/${encodeURIComponent(data.workspaceId)}/tickets/${encodeURIComponent(relation.target_resource_key)}`}>
                <span>{relationLabel(relation.kind)}</span>
                <strong>{relation.target_resource_key}</strong>
                {#if relation.note}<small>{relation.note}</small>{/if}
              </a>
            {:else}
              <span><strong>resource key unavailable</strong></span>
            {/if}
          {/each}
          {#each ticket.relations.incoming as relation}
            {#if relation.source_resource_key}
              <a href={`/w/${encodeURIComponent(data.workspaceId)}/tickets/${encodeURIComponent(relation.source_resource_key)}`}>
                <span>{relationLabel(relation.inverse_kind)}</span>
                <strong>{relation.source_resource_key}</strong>
                {#if relation.note}<small>{relation.note}</small>{/if}
              </a>
            {:else}
              <span><strong>resource key unavailable</strong></span>
            {/if}
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
        <header><h2>Role assignments</h2><span>Server-authoritative</span></header>
        {#if ticket.assignments.length > 0}
          <ul class="ticket-assignment-list">
            {#each ticket.assignments as assignment}
              <li>
                <strong>{assignment.role}</strong>
                <span>
                  {#if assignment.principal.kind === "worker"}
                    {assignment.principal.runtime_id}/{assignment.principal.worker_id}
                  {:else if assignment.principal.kind === "user"}
                    {assignment.principal.account_id}
                  {:else}
                    {assignment.principal.agent_key}
                  {/if}
                </span>
              </li>
            {/each}
          </ul>
        {:else}
          <p class="workspace-empty-copy">No active role assignment.</p>
        {/if}
        {#if ticket.action_eligibility.can_assign_orchestrator}
          <button
            class="workspace-primary-button"
            type="button"
            disabled={busy !== null}
            onclick={assignOrchestrator}
          >
            {busy === "assign-orchestrator" ? "Assigning…" : "Assign Orchestrator"}
          </button>
        {/if}
        {#if implementationStartEligible}
          <form class="ticket-control-form" onsubmit={startManualCoder}>
            <label>Runtime ID<input bind:value={manualRuntimeId} required /></label>
            <label>Worker ID<input bind:value={manualWorkerId} required /></label>
            <button
              class="workspace-secondary-button"
              type="submit"
              disabled={busy !== null || !manualRuntimeId.trim() || !manualWorkerId.trim()}
            >
              {busy === "start-manual" ? "Starting…" : "Assign Coder and start"}
            </button>
          </form>
        {/if}
        {#if ticket.state === "inprogress" && coderAssignment}
          <details class="ticket-cancel-implementation">
            <summary>Cancel implementation</summary>
            <form class="ticket-control-form" onsubmit={cancelImplementation}>
              <p class="workspace-empty-copy">
                Cancel the assigned Coder, remove its assignment, and return this Ticket to ready.
              </p>
              <label>Reason<textarea bind:value={cancellationReason} rows="3" required></textarea></label>
              <button
                class="workspace-danger-button"
                type="submit"
                disabled={busy !== null || !cancellationReason.trim()}
              >
                {busy === "cancel-implementation" ? "Cancelling…" : "Cancel and return to ready"}
              </button>
            </form>
          </details>
        {/if}
        {#if ticket.assignment_diagnostics.length > 0}
          {#each ticket.assignment_diagnostics as diagnostic}
            <p class="workspace-callout">{diagnostic}</p>
          {/each}
        {/if}
      </section>

      <section class="ticket-control-card">
        <header><h2>Repository targets</h2><span>{targetDrafts.length}</span></header>
        <form class="ticket-control-form" onsubmit={saveTarget}>
          <div class="ticket-target-list">
            {#each targetDrafts as target, index}
              {@const selectedRepository = repositoryFor(target.repository_key)}
              <fieldset class="ticket-target-row">
                <legend>Target {index + 1}</legend>
                <label>Repository
                  <select bind:value={target.repository_key} disabled={ticket.state !== "planning"} required>
                    <option value="">Choose repository</option>
                    {#each loadedRepositories?.items ?? [] as repository}
                      <option value={repository.repository_key}>{repository.repository_key}</option>
                    {/each}
                  </select>
                </label>
                <label>Ref selector<input bind:value={target.ref_selector} placeholder={selectedRepository?.default_selector ?? "branch, tag, or revision"} disabled={ticket.state !== "planning"} /></label>
                <label>Access
                  <select bind:value={target.access} disabled={ticket.state !== "planning"}>
                    <option value="read_write">Read and write</option>
                    <option value="read_only">Read only</option>
                  </select>
                </label>
                {#if ticket.state === "planning"}
                  <button class="workspace-secondary-button" type="button" onclick={() => removeTarget(index)}>Remove target</button>
                {/if}
              </fieldset>
            {:else}
              <p class="workspace-empty-copy">No repository targets.</p>
            {/each}
          </div>
          {#if ticket.state === "planning"}
            <button
              class="workspace-secondary-button"
              type="button"
              onclick={() => addTarget(targetDrafts.some((target) => target.access === "read_write") ? "read_only" : "read_write")}
            >Add target</button>
          {/if}
          <button class="workspace-secondary-button" type="submit" disabled={busy === "target" || ticket.state !== "planning"}>
            {busy === "target" ? "Saving…" : "Save targets"}
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
            <p class="workspace-empty-copy">Add unique healthy repository targets with effective ref selectors and exactly one read-write target before marking ready.</p>
          {/if}
        {:else if ticket.state === "ready"}
          <button class="workspace-primary-button ticket-queue-button" type="button" disabled={busy === "queue" || !ticket.action_eligibility.can_queue} onclick={() => void queueTicket()}>
            {busy === "queue" ? "Queueing…" : `Queue ${ticket.action_eligibility.queue_tickets.length} Ticket(s)`}
          </button>
          {#if !ticket.action_eligibility.can_queue}
            <p class="workspace-empty-copy">Queue requires a valid target, an active Orchestrator assignment, no active Coder assignment, and no dependency still in planning.</p>
          {:else if ticket.action_eligibility.queue_tickets.length > 0}
            <p class="workspace-empty-copy">This operation queues: {ticket.action_eligibility.queue_tickets.join(", ")}.</p>
            {#if ticket.relations.blockers.length > 0}
              <p class="workspace-empty-copy">Ready dependencies are queued atomically. Queued or in-progress dependencies remain unchanged for the Orchestrator to schedule.</p>
            {/if}
          {/if}
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
          <p><strong>{mergeRequest.state}</strong></p>
          <p>
            From <code>{mergeRequest.selector_from ?? "requires repair"}</code>
            to <code>{mergeRequest.selector_to}</code>
          </p>
          {#if mergeRequest.current_subject_ref && mergeRequest.review_subject_ref === mergeRequest.current_subject_ref}
            <p><strong>Source review:</strong> {mergeRequest.review_status} for exact ref <code>{mergeRequest.current_subject_ref}</code></p>
          {:else if mergeRequest.current_subject_ref && mergeRequest.review_subject_ref}
            <p><strong>Fresh source review required:</strong> selector_from moved from <code>{mergeRequest.review_subject_ref}</code> to <code>{mergeRequest.current_subject_ref}</code>.</p>
          {:else if mergeRequest.current_subject_ref}
            <p><strong>Fresh source review required:</strong> no effective verdict exists for <code>{mergeRequest.current_subject_ref}</code>.</p>
          {:else}
            <p><strong>Source review unavailable:</strong> selector_from is unresolved.</p>
          {/if}
          <p><strong>Target integration:</strong> {mergeRequest.state === "merged" ? "recorded" : `awaiting Orchestrator integration into ${mergeRequest.selector_to}`}.</p>
          <p class="workspace-empty-copy">Target-only movement refreshes integration evidence; it does not invalidate approval for an unchanged source.</p>
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
