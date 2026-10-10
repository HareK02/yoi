<script lang="ts">
  import { untrack } from "svelte";
  import RichMarkdown from "#lib/workspace/console/RichMarkdown.svelte";
  import {
    workspaceApiJson,
    workspaceApiJsonWithBody,
    workspaceApiPath,
  } from "#lib/workspace/api/http.ts";
  import {
    TICKET_BROWSER_API_MAX_RESPONSE_BYTES,
    parseTicketDetail,
    parseTicketQueueOutcome,
    parseTicketRoleAssignmentMutationResponse,
  } from "#lib/workspace/api/ticket-browser.ts";
  import { mergeRequestPagePath } from "#lib/workspace/api/merge-requests.ts";
  import { currentRequirementApprovalStatus, summarySourceReviewStatus } from "#lib/workspace/merge-request-status.ts";
  import {
    relationLabel,
    TICKET_STATES,
    ticketWorkerLaunchHref,
    type WorkspaceOrchestratorStatus,
  } from "#lib/workspace/tickets/ticket-panel.ts";
  import type { ApiResult } from "#lib/workspace/api/http.ts";
  import type {
    TicketTarget,
    TicketTargetAccess,
  } from "#lib/generated/ticket-api.ts";
  import type {
    RepositoryListResponse,
    RepositorySummary,
    TicketDetail,
  } from "#lib/workspace/sidebar/types.ts";

  type EditableTicketTarget = {
    repository_key: string;
    ref_selector: string;
    access: TicketTargetAccess;
  };

  const MUTABLE_TICKET_STATES = TICKET_STATES;

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
  const mergeRequests = $derived(ticket.merge_requests);
  let editing = $state(false);
  let editTitle = $state(loadedTicket.title);
  let editBody = $state(loadedTicket.body);
  let targetDrafts = $state<EditableTicketTarget[]>(
    editableTargets(loadedTicket.targets),
  );
  let nextState = $state(loadedTicket.state);
  let transitionReason = $state("");
  let progressBody = $state("");
  let threadRole = $state("comment");
  let threadBody = $state("");
  let resolution = $state("");
  let busy = $state<string | null>(null);
  let errorMessage = $state<string | null>(null);
  let queueMessage = $state<string | null>(null);
  let readyOperationKey = $state<string | null>(null);
  let manualRuntimeId = $state("");
  let manualWorkerId = $state("");
  let manualBindings = $state<{ alias: string; working_directory_id: string; connection_id: string }[]>([]);
  let cancellationReason = $state("");
  let routeTicketSnapshot = JSON.stringify([initialData.workspaceId, initialData.ticketId, loadedTicket]);
  let routeGeneration = 0;
  const workerAssignment = $derived(
    ticket.assignments.find((assignment) => assignment.role === "worker") ?? null,
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

    const repositoryKeys = new Set<string>();

    for (const target of targetDrafts) {
      const repository = repositoryFor(target.repository_key);
      if (
        !target.repository_key || repository === null ||
        repositoryKeys.has(target.repository_key)
      ) return false;
      repositoryKeys.add(target.repository_key);

    }
    return true;
  });
  const implementationStartEligible = $derived(
    ticket.action_eligibility.can_start_manual_worker,
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
    progressBody = "";
    threadRole = "comment";
    threadBody = "";
    resolution = "";
    busy = null;
    errorMessage = null;
    queueMessage = null;
    readyOperationKey = null;
    manualRuntimeId = "";
    manualWorkerId = "";
    manualBindings = [];
    cancellationReason = "";
  }

  $effect(() => {
    const incomingTicketId = data.ticketId;
    const incomingTicket = data.ticket.data;
    if (!incomingTicket) return;
    const incomingSnapshot = JSON.stringify([data.workspaceId, incomingTicketId, incomingTicket]);

    untrack(() => {
      if (incomingSnapshot === routeTicketSnapshot) return;
      routeTicketSnapshot = incomingSnapshot;
      routeGeneration += 1;
      resetTicketView(incomingTicket);
    });
  });

  function ticketMutationError(error: unknown): string {
    const message = error instanceof Error ? error.message : String(error);
    if (/\(409\)/.test(message)) return `Update conflict. Refresh the Ticket and resolve the current content or resource binding before retrying. ${message}`;
    if (/\(401\)|\(403\)/.test(message)) return `Permission denied. This action did not grant access. ${message}`;
    if (/\(404\)/.test(message)) return `Resource unavailable or not connected. Check the Ticket and authorized resource connection. ${message}`;
    return `Outcome unknown or request rejected. Refresh before retrying; no success is inferred. ${message}`;
  }

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
        errorMessage = ticketMutationError(error);
      }
      return false;
    } finally {
      if (generation === routeGeneration) busy = null;
    }
  }

  async function refreshTicket(): Promise<void> {
    if (busy) return;
    const generation = routeGeneration;
    busy = "refresh";
    try {
      const updated = await workspaceApiJson(ticketPath, parseTicketDetail, TICKET_BROWSER_API_MAX_RESPONSE_BYTES);
      if (generation !== routeGeneration) return;
      applyTicket(updated);
      errorMessage = null;
    } catch (error) {
      if (generation === routeGeneration) errorMessage = ticketMutationError(error);
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
        errorMessage = ticketMutationError(error);
      }
    } finally {
      if (generation === routeGeneration) busy = null;
    }
  }

  async function mutateAssignment(
    action: string,
    role: "orchestrator" | "worker",
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
            ...(role === "worker" ? { workdir_bindings: manualBindings } : {}),
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
        errorMessage = ticketMutationError(error);
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

  async function assignExistingWorker(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (!manualRuntimeId.trim() || !manualWorkerId.trim()) return;
    await mutateAssignment("start-manual", "worker", {
      kind: "worker",
      runtime_id: manualRuntimeId.trim(),
      worker_id: manualWorkerId.trim(),
    });
  }

  async function cancelImplementation(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (!workerAssignment || !cancellationReason.trim()) return;
    if (
      await mutate("cancel-implementation", "/implementation-cancellations", {
        operation_id: crypto.randomUUID(),
        assignment_id: workerAssignment.assignment_id,
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
    targetDrafts.push({ repository_key: "", ref_selector: "", access });
  }

  function removeTarget(index: number): void {
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
    if (ticket.state !== "planning" || busy) return;
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
        operation_key: crypto.randomUUID(),
        expected_content_digest: ticket.content_digest,
        expected_state: ticket.state,
        reason: transitionReason.trim(),
        body: progressBody.trim() || null,
      })
    ) { transitionReason = ""; progressBody = ""; }
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
      await mutate("close", "/close", {
        resolution: resolution.trim(),
        operation_key: crypto.randomUUID(),
        expected_content_digest: ticket.content_digest,
        expected_state: ticket.state,
      })
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
    <div class="workspace-callout is-error" role="alert"><p>{errorMessage}</p><p>No success or approval is inferred. An uncertain response may already have applied; refresh the current Ticket before retrying.</p><button type="button" class="workspace-secondary-button" disabled={busy !== null} onclick={refreshTicket}>Refresh Ticket status</button></div>
  {/if}

  {#if queueMessage}
    <div class="workspace-callout" role="status">{queueMessage}</div>
  {/if}

  {#if editing}
    <form class="ticket-editor" onsubmit={saveEdit}>
      <label>Title<input bind:value={editTitle} required /></label>
      <label>Body<textarea bind:value={editBody} rows="12"></textarea></label>
      <button class="workspace-primary-button" type="submit" disabled={busy !== null || !editTitle.trim()}>
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
                {#if event.reason}<RichMarkdown text={event.reason} />{/if}
                {#if event.body}<RichMarkdown text={event.body} />{/if}
                {#each event.references as reference}<p>{reference}</p>{/each}
              </div>
            </article>
          {:else}
            <p class="workspace-empty-copy">No timeline events.</p>
          {/each}
        </div>
      </section>
    </main>

    <aside class="ticket-control-rail">
      <section class="ticket-control-card">
        <header><h2>Progress decision</h2></header>
        <p class="workspace-empty-copy">State records progress only. It does not start or stop a Worker, grant access, approve review, or merge code. Reopening does not resume an old Worker.</p>
        <form class="ticket-control-form" onsubmit={transition}>
          <label>State
            <select bind:value={nextState}>
              {#each MUTABLE_TICKET_STATES as state}<option value={state}>{state}</option>{/each}
            </select>
          </label>
          <label>Reason<input bind:value={transitionReason} placeholder="Completion, start, or reopening decision" required /></label>
          <label>Result, references, and remaining work (optional)<textarea bind:value={progressBody} rows="3" placeholder="A comment can be the result. Add links and remaining questions here."></textarea></label>
          <button class="workspace-secondary-button" type="submit" disabled={busy !== null || nextState === ticket.state || !transitionReason.trim()}>
            {nextState === "done" ? "Complete Ticket" : ticket.state === "done" || ticket.state === "closed" ? "Reopen / apply state" : "Apply state"}
          </button>
        </form>
        {#if ticket.state === "planning"}
          <button class="workspace-secondary-button ticket-queue-button" type="button" disabled={busy !== null} onclick={markReady}>{busy === "ready" ? "Marking ready…" : "Mark ready"}</button>
        {/if}
        <button class="workspace-secondary-button ticket-queue-button" type="button" disabled={busy !== null || !ticket.action_eligibility.can_queue} onclick={() => void queueTicket()}>{busy === "queue" ? "Queueing…" : "Request Orchestrator (queue)"}</button>
        <p class="workspace-empty-copy">Queue requests only this Ticket. Dependencies remain diagnostic information and are not automatically queued.</p>
        {#each ticket.action_eligibility.blockers as blocker}<p class="workspace-callout">{blocker}</p>{/each}
      </section>

      <section class="ticket-control-card ticket-worker-card">
        <header><h2>Role assignments</h2><span>Retained responsibility</span></header>
        <p class="workspace-empty-copy">Responsibility is retained after work ends and Worker removal; it does not indicate a running Worker or active work authority.</p>
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
          <p class="workspace-empty-copy">No role responsibility recorded.</p>
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
          <a class="workspace-secondary-button" href={ticketWorkerLaunchHref(data.workspaceId, ticket)}>Start a Ticket Worker</a>
          <details><summary>Assign an existing Worker</summary>
          <p class="workspace-empty-copy">Explicit assignment is separate from state. Backend validates the Worker and resource binding; a retained assignment is not execution authority.</p>
          <form class="ticket-control-form" onsubmit={assignExistingWorker}>
            <label>Runtime ID<input bind:value={manualRuntimeId} required /></label>
            <label>Worker ID<input bind:value={manualWorkerId} required /></label>
            <p class="workspace-empty-copy">Select only resources needed for this request. No selection binds no Workdirs; unrelated existing attachments are not inherited. Use authorized connection IDs for explicit rebind.</p>
            {#each manualBindings as binding, index}
              <fieldset class="ticket-target-row"><legend>Resource binding {index + 1}</legend>
                <label>Alias<input bind:value={binding.alias} required /></label>
                <label>Workdir ID<input bind:value={binding.working_directory_id} required /></label>
                <label>Connection ID<input bind:value={binding.connection_id} required /></label>
                <button type="button" class="workspace-secondary-button" disabled={busy !== null} onclick={() => manualBindings.splice(index, 1)}>Remove binding</button>
              </fieldset>
            {/each}
            <button type="button" class="workspace-secondary-button" disabled={busy !== null} onclick={() => manualBindings.push({ alias: "", working_directory_id: "", connection_id: "" })}>Add resource binding</button>
            <button
              class="workspace-secondary-button"
              type="submit"
              disabled={busy !== null || !manualRuntimeId.trim() || !manualWorkerId.trim()}
            >
              {busy === "start-manual" ? "Assigning…" : "Assign Worker"}
            </button>
          </form></details>
        {/if}
        {#if ticket.state === "inprogress" && workerAssignment}
          <details class="ticket-cancel-implementation">
            <summary>Cancel implementation</summary>
            <form class="ticket-control-form" onsubmit={cancelImplementation}>
              <p class="workspace-empty-copy">
                Cancel the assigned Worker’s unfinished work and return this Ticket to ready. Responsibility is retained. This does not delete the Worker or Workdir.
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
        <header><h2>Repository resources (optional)</h2><span>{targetDrafts.length}</span></header>
        <p class="workspace-empty-copy">Targets describe permitted resources, not connected attachments. Editing targets or the body does not expand live permissions. Changed resources require explicit reconnection; stale or unavailable connections are rejected by Backend.</p>
        {#if data.repositories.error}<p class="workspace-callout is-error">Repository catalog unavailable: {data.repositories.error}</p>{/if}
        <form class="ticket-control-form" onsubmit={saveTarget}>
          <div class="ticket-target-list">
            {#each targetDrafts as target, index}
              {@const selectedRepository = repositoryFor(target.repository_key)}
              <fieldset class="ticket-target-row">
                <legend>Target {index + 1}</legend>
                <label>Repository
                  <select bind:value={target.repository_key} disabled={busy !== null} required>
                    <option value="">Choose repository</option>
                    {#each loadedRepositories?.items ?? [] as repository}
                      <option value={repository.repository_key}>{repository.repository_key}</option>
                    {/each}
                  </select>
                </label>
                <label>Ref selector<input bind:value={target.ref_selector} placeholder={selectedRepository?.default_selector ?? "branch, tag, or commit"} disabled={busy !== null} /></label>
                <label>Access
                  <select bind:value={target.access} disabled={busy !== null}>
                    <option value="read_write">Read and write</option>
                    <option value="read_only">Read only</option>
                  </select>
                </label>
                  <button class="workspace-secondary-button" type="button" onclick={() => removeTarget(index)}>Remove target</button>
              </fieldset>
            {:else}
              <p class="workspace-empty-copy">No repository targets.</p>
            {/each}
          </div>
            <button
              class="workspace-secondary-button"
              type="button"
              onclick={() => addTarget()}
            >Add target</button>
          <button class="workspace-secondary-button" type="submit" disabled={busy !== null || !targetCandidateValid}>
            {busy === "target" ? "Saving…" : "Save targets"}
          </button>
        </form>
      </section>



      <details class="ticket-control-card">
        <summary>Record result or comment</summary>
        <form class="ticket-control-form" onsubmit={appendThread}>
          <label>Role<select bind:value={threadRole}>
            <option value="comment">Comment</option>
            <option value="plan">Plan</option>
            <option value="decision">Decision</option>
            <option value="implementation_report">Implementation report</option>
          </select></label>
          <label>Body<textarea bind:value={threadBody} rows="5" required></textarea></label>
          <button class="workspace-secondary-button" type="submit" disabled={busy !== null || !threadBody.trim()}>Append event</button>
        </form>
      </details>

      {#if mergeRequests.length > 0}
      <section class="ticket-control-card">
        <header><h2>MR requirement evidence</h2></header>
        <p><strong>Current requirements:</strong> {ticket.evidence.complete_for_integration ? "satisfied for integration" : "not satisfied for integration"}.</p>
        <p><strong>Current requirement approval:</strong> {currentRequirementApprovalStatus(ticket.evidence)}.</p>
        {#if ticket.evidence.missing.length > 0}
          <ul>
            {#each ticket.evidence.missing as missing}<li>{missing}</li>{/each}
          </ul>
        {/if}
        <p class="workspace-empty-copy">This evaluates linked MR requirements, separately from Ticket progress and persisted integration approval.</p>
        {#if ticket.state === "done"}
          <p class="workspace-empty-copy">This Ticket remains done. Incomplete current requirement evidence does not cancel recorded completion.</p>
        {/if}
      </section>

      {/if}
      <section class="ticket-control-card">
        <header><h2>Merge Requests</h2></header>
        <p class="workspace-empty-copy">Ticket completion is not MR approval or integration.</p>
        {#if mergeRequests.length > 0}
          {#each mergeRequests as mergeRequest (mergeRequest.merge_request_id)}
            <article class="ticket-control-card">
              <p><strong>{mergeRequest.repository_key}: {mergeRequest.state}</strong></p>
              <p>
                From <code>{mergeRequest.selector_from ?? "requires repair"}</code>
                to <code>{mergeRequest.selector_to}</code>
              </p>
              <p><strong>{mergeRequest.state === "merged" ? "Immutable integration evidence" : "Live source review"}:</strong> {summarySourceReviewStatus(mergeRequest)}</p>
              {#if mergeRequest.integration_evidence_error !== null}
                <p class="workspace-callout is-error"><strong>Integration evidence error:</strong> {mergeRequest.integration_evidence_error}</p>
              {/if}
              <p><strong>Target integration:</strong> {mergeRequest.state === "merged" ? "recorded" : `awaiting Orchestrator integration into ${mergeRequest.selector_to}`}.</p>
              {#if mergeRequest.state === "merged"}
                <p class="workspace-empty-copy">The merged source and integration approval are persisted evidence, not a live selector observation.</p>
              {:else}
                <p class="workspace-empty-copy">Target-only movement refreshes integration evidence; it does not invalidate approval for an unchanged source.</p>
              {/if}
              <a
                class="workspace-secondary-button"
                href={mergeRequestPagePath(data.workspaceId, mergeRequest.merge_request_id)}
              >Open Merge Request</a>
            </article>
          {/each}
        {:else}
          <p class="workspace-empty-copy">No Merge Requests linked. A Ticket can be completed with a comment or document result.</p>
        {/if}
      </section>

      {#if ticket.state !== "closed"}
        <details class="ticket-control-card ticket-close-card">
          <summary>Close ticket</summary>
          <form class="ticket-control-form" onsubmit={closeTicket}>
            <label>Resolution<textarea bind:value={resolution} rows="5" required></textarea></label>
            <button class="workspace-danger-button" type="submit" disabled={busy !== null || !resolution.trim()}>Close ticket</button>
          </form>
        </details>
      {:else if ticket.resolution}
        <section class="ticket-control-card"><header><h2>Resolution</h2></header><RichMarkdown text={ticket.resolution} /></section>
      {/if}
    </aside>
  </div>
</div>
