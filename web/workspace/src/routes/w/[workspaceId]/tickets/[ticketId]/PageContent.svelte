<script lang="ts">
  import { untrack } from "svelte";
  import TicketPost from "#lib/workspace/tickets/TicketPost.svelte";
  import TicketTags from "#lib/workspace/tickets/TicketTags.svelte";
  import TicketWorkerObservation from "#lib/workspace/tickets/TicketWorkerObservation.svelte";
  import TicketHandling from "#lib/workspace/tickets/TicketHandling.svelte";
  import { observeOperation, type ObservedOperation } from "#lib/workspace/api/ticket-operations.ts";
  import RichMarkdown from "#lib/workspace/console/RichMarkdown.svelte";
  import BevelLine from "#lib/workspace/ui/BevelLine.svelte";
  import {
    workspaceApiJson,
    workspaceApiJsonWithBody,
    workspaceApiPath,
  } from "#lib/workspace/api/http.ts";
  import {
    TICKET_BROWSER_API_MAX_RESPONSE_BYTES,
    parseTicketSummary,
    parseTicketDetail,
    parseTicketEvent,
    parseTicketRoleAssignmentMutationResponse,
  } from "#lib/workspace/api/ticket-browser.ts";
  import { ticketCaseRequest } from "#lib/workspace/api/ticket-case.ts";
  import { mergeRequestPagePath } from "#lib/workspace/api/merge-requests.ts";
  import { currentRequirementApprovalStatus, summarySourceReviewStatus } from "#lib/workspace/merge-request-status.ts";
  import {
    relationLabel,
    assignmentPrincipalLabel,
    ticketWorkerLaunchHref,
  } from "#lib/workspace/tickets/ticket-panel.ts";
  import type {
    TicketCaseAction,
    TicketEventDetail,
    TicketPostEditRequest,
    TicketClosureReason,
    TicketTarget,
    TicketTargetAccess,
  } from "#lib/generated/ticket-api.ts";
  import type {
    RepositorySummary,
    TicketDetail,
  } from "#lib/workspace/sidebar/types.ts";
  import type { PageData as RouteData } from "./$types";
  import type { ResolvedPageData } from "#lib/workspace/navigation/deferred.ts";

  type PageData = ResolvedPageData<RouteData>;

  type EditableTicketTarget = {
    repository_key: string;
    ref_selector: string;
    access: TicketTargetAccess;
  };


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

  const { data } = $props<{ data: PageData }>();

  const initialData = untrack(() => data);
  const loadedTicket = initialData.ticket.data;
  if (!loadedTicket) throw new Error(initialData.ticket.error ?? "ticket load failed");
  const loadedRepositories = $derived(data.repositories.data);

  let ticket = $state<TicketDetail>(loadedTicket);
  const mergeRequests = $derived(ticket.merge_requests);
  let editing = $state(false);
  let editTitle = $state(loadedTicket.title);
  let targetDrafts = $state<EditableTicketTarget[]>(
    editableTargets(loadedTicket.targets),
  );
  let closureReason = $state("unknown");
  let otherClosureReason = $state("");
  let threadRole = $state("comment");
  let threadBody = $state("");
  let busy = $state<string | null>(null);
  let errorMessage = $state<string | null>(null);
  let caseOperation = $state<ObservedOperation | null>(null);
  let caseSaved = $state(false);
  let caseComment = $state<string | null>(null);
  let commentError = $state<string | null>(null);
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
    targetDrafts = editableTargets(updatedTicket.targets);
  }

  function resetTicketView(updatedTicket: TicketDetail): void {
    applyTicket(updatedTicket);
    editing = false;
    caseComment = null;
    commentError = null;
    closureReason = "unknown";
    otherClosureReason = "";
    threadRole = "comment";
    threadBody = "";
    busy = null;
    errorMessage = null;
    caseOperation = null;
    caseSaved = false;
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

  async function refreshOperationTicket(): Promise<void> {
    const generation = routeGeneration;
    const updated = await workspaceApiJson(ticketPath, parseTicketDetail, TICKET_BROWSER_API_MAX_RESPONSE_BYTES);
    if (generation !== routeGeneration) return;
    applyTicket(updated);
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
            expected_assignment_id: ticket.assignments.find((row) => row.role === role)?.assignment_id ?? null,
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

  function selectedClosureReason(): TicketClosureReason {
    return closureReason === "other" ? { other: otherClosureReason.trim() } : closureReason as TicketClosureReason;
  }

  async function caseDecision(action: TicketCaseAction): Promise<void> {
    if (busy || caseOperation || threadRole !== "comment" || !threadBody.trim()) return;
    if (action.action !== "reopen" && closureReason === "other" && !otherClosureReason.trim()) return;
    // Closeの本文と案件状態は既存APIが同時に保存する。別のコメントPOSTで二重記録しない。
    const request = ticketCaseRequest(ticket, action, threadBody, crypto.randomUUID());
    caseComment = threadBody;
    const path = `/workspaces/${encodeURIComponent(data.workspaceId)}/tickets/${encodeURIComponent(data.ticketId)}/case`;
    await runCaseDecision(observeOperation(path, "POST", request));
  }

  async function runCaseDecision(operation: ObservedOperation): Promise<void> {
    if (busy) return;
    const generation = routeGeneration;
    const path = ticketPath;
    busy = "case";
    caseOperation = operation;
    commentError = null;
    caseSaved = false;
    try {
      const saved = await workspaceApiJsonWithBody(operation.path, { method: operation.method, body: operation.body }, parseTicketSummary, TICKET_BROWSER_API_MAX_RESPONSE_BYTES);
      if (generation !== routeGeneration) return;
      caseOperation = null;
      caseSaved = true;
      ticket = { ...ticket, state: saved.workflow_state };
      if (threadBody === caseComment) threadBody = "";
      caseComment = null;
      const updated = await workspaceApiJson(path, parseTicketDetail, TICKET_BROWSER_API_MAX_RESPONSE_BYTES);
      if (generation !== routeGeneration) return;
      applyTicket(updated);
    } catch (error) {
      if (generation === routeGeneration) commentError = caseSaved
        ? "Comment and case change saved, but the timeline could not be refreshed. Refresh the thread."
        : ticketMutationError(error);
    } finally {
      if (generation === routeGeneration) busy = null;
    }
  }

  async function refreshCommentThread(): Promise<void> {
    const generation = routeGeneration;
    await refreshTicket();
    if (generation !== routeGeneration) return;
    commentError = errorMessage;
    errorMessage = null;
  }

  async function reconcileCase(): Promise<void> {
    if (busy) return;
    const generation = routeGeneration;
    await refreshCommentThread();
    if (generation !== routeGeneration) return;
    if (!commentError) { caseOperation = null; caseComment = null; }
  }

  function closureReasonLabel(reason: TicketClosureReason): string {
    return typeof reason === "string" ? relationLabel(reason) : reason.other;
  }

  async function appendThread(event: SubmitEvent) {
    event.preventDefault();
    if (busy || caseOperation || !threadBody.trim()) return;
    const generation = routeGeneration;
    const comment = threadBody;
    commentError = null;
    caseSaved = false;
    const saved = await mutate("thread", "/events", {
      role: threadRole,
      body: comment.trim(),
    });
    if (generation !== routeGeneration) return;
    if (saved && threadBody === comment) threadBody = "";
    if (!saved) { commentError = errorMessage; errorMessage = null; }
  }

  async function loadPost(post: TicketEventDetail): Promise<TicketEventDetail> {
    return await workspaceApiJson(`${ticketPath}/posts/${encodeURIComponent(post.event_ref)}`, parseTicketEvent, TICKET_BROWSER_API_MAX_RESPONSE_BYTES);
  }

  async function savePost(post: TicketEventDetail, body: string): Promise<void> {
    if (busy || post.body_truncated || !post.editable || post.kind !== "comment") throw new Error("Post cannot be saved now.");
    const generation = routeGeneration;
    const request: TicketPostEditRequest = { body, expected_body_digest: post.body_digest };
    busy = "post";
    try {
      const updated = await workspaceApiJsonWithBody(`${ticketPath}/posts/${encodeURIComponent(post.event_ref)}`, {
        method: "PATCH", body: JSON.stringify(request),
      }, parseTicketDetail, TICKET_BROWSER_API_MAX_RESPONSE_BYTES);
      if (generation !== routeGeneration) return;
      applyTicket(updated);
    } finally {
      if (generation === routeGeneration) busy = null;
    }
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
        <span>{ticket.resource_key}</span>
        <span class="workspace-status-pill" data-status={ticket.state}>Case: {ticket.state}</span>
      </div>
      <h1>{ticket.title}</h1>
      {#if ticket.state === "closed" && ticket.current_closure}<p><strong>Closure reason:</strong> {closureReasonLabel(ticket.current_closure.reason)}</p>{/if}
    </div>
  </header>

  {#if errorMessage}
    <div class="workspace-callout is-error" role="alert"><p>{errorMessage}</p><p>No success or approval is inferred. An uncertain response may already have applied; refresh the current Ticket before retrying.</p><button type="button" class="workspace-secondary-button" disabled={busy !== null} onclick={refreshTicket}>Refresh Ticket status</button></div>
  {/if}




  <BevelLine decorative class="ticket-header-divider" />
  <div class="ticket-detail-grid">
    <main class="ticket-detail-main" aria-label="Ticket thread">
      <section class="ticket-thread">
        <div class="ticket-timeline">
          {#key JSON.stringify([data.workspaceId, ticket.id])}
          {#each [ticket.initial_post, ...ticket.events] as post, index (post.event_ref)}
            {#if index > 0}<BevelLine decorative />{/if}
            <TicketPost {post} busy={busy !== null} load={loadPost} save={savePost} />
          {/each}
          {/key}
        </div>
        <form class="ticket-control-form ticket-thread-composer" aria-label="Add a comment" onsubmit={appendThread}>
          <BevelLine decorative />
          <label>Comment<textarea bind:value={threadBody} rows="5" required disabled={busy !== null}></textarea></label>
          {#if commentError}
            <div class="workspace-callout is-error" role="alert">
              <p>{commentError}</p>
              {#if !caseOperation}<button type="button" class="workspace-secondary-button" disabled={busy !== null} onclick={refreshCommentThread}>Refresh thread</button>{/if}
            </div>
          {/if}
          {#if caseSaved}<p role="status">Comment and case change saved.</p>{/if}
          {#if caseOperation}
            <p role="status">The comment and case change may already be saved. Retry the same request or refresh before continuing.</p>
            <div class="ticket-comment-actions">
              <button type="button" class="workspace-secondary-button" disabled={busy !== null} onclick={() => caseOperation && void runCaseDecision(caseOperation)}>Retry exact case decision</button>
              <button type="button" class="workspace-secondary-button" disabled={busy !== null} onclick={reconcileCase}>Refresh and reconcile case</button>
            </div>
          {/if}
          <div class="ticket-comment-actions">
            <button class="workspace-primary-button" type="submit" disabled={busy !== null || caseOperation !== null || !threadBody.trim()}>{busy === "thread" ? "Saving…" : threadRole === "comment" ? "Add comment" : "Append event"}</button>
            {#if threadRole === "comment"}
              <button class="workspace-secondary-button" type="button" disabled={busy !== null || caseOperation !== null || !threadBody.trim() || (ticket.state === "open" && closureReason === "other" && !otherClosureReason.trim())} onclick={() => void caseDecision(ticket.state === "open" ? { action: "close", reason: selectedClosureReason() } : { action: "reopen" })}>
                {busy === "case" ? "Saving…" : ticket.state === "open" ? "Comment and close" : "Reopen ticket"}
              </button>
            {/if}
          </div>
          {#if threadRole === "comment"}
            <details class="ticket-close-options">
              <summary>{ticket.state === "open" ? `Close reason: ${closureReasonLabel(selectedClosureReason())}` : "Correct closure"}</summary>
              <label>Closure reason<select bind:value={closureReason} disabled={busy !== null || caseOperation !== null}>
                <option value="completed">Completed</option>
                <option value="not_planned">Not planned</option>
                <option value="duplicate">Duplicate</option>
                <option value="unknown">Unknown</option>
                <option value="other">Other</option>
              </select></label>
              {#if closureReason === "other"}<label>Other reason<input bind:value={otherClosureReason} disabled={busy !== null || caseOperation !== null} /></label>{/if}
              {#if ticket.state === "closed" && ticket.current_closure}
                <button class="workspace-secondary-button" type="button" disabled={busy !== null || caseOperation !== null || !threadBody.trim() || (closureReason === "other" && !otherClosureReason.trim())} onclick={() => void caseDecision({ action: "correct_closure", reason: selectedClosureReason() })}>Correct closure</button>
              {/if}
            </details>
          {/if}
          <details class="ticket-record-type"><summary>Record type</summary>
            <label>Role<select bind:value={threadRole} disabled={busy !== null || caseOperation !== null}>
              <option value="comment">Comment</option>
              <option value="plan">Plan</option>
              <option value="decision">Decision</option>
              <option value="implementation_report">Implementation report</option>
            </select></label>
          </details>
        </form>
      </section>
    </main>

    <div class="ticket-column-divider"><BevelLine direction="y" length="100%" decorative /></div>
    <aside class="ticket-control-rail" aria-label="Ticket details">
      <div class="ticket-mobile-sidebar-divider"><BevelLine decorative /></div>
      <section class="ticket-control-card ticket-resources" aria-label="Repository resources">
        <div class="ticket-section-heading"><h2>Repository resources</h2><span>{ticket.targets.length}</span></div>
        <ul class="ticket-resource-summary">
          {#each ticket.targets as target}
            <li>
              <strong>{target.repository_key}</strong>
              <span class="ticket-resource-access">{target.access === "read_write" ? "Read/write" : "Read only"}</span>
              <code>{target.ref_selector ?? "Ref not specified"}</code>
            </li>
          {:else}
            <li class="workspace-empty-copy">No repository targets.</li>
          {/each}
        </ul>
        {#if data.repositories.error}<p class="workspace-callout is-error">Repository catalog unavailable: {data.repositories.error}</p>{/if}
        <details class="ticket-resource-editor">
          <summary>Edit resources</summary>
          <p class="workspace-empty-copy">Target changes do not reconnect Workers or change live permissions.</p>
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
        </details>
      </section>
      <BevelLine decorative />
      {#each ticket.action_eligibility.blockers as blocker}<p class="workspace-callout">{blocker}</p>{/each}
      {#key JSON.stringify([data.workspaceId, ticket.id])}
        <TicketHandling workspaceId={data.workspaceId} {ticket} refresh={refreshOperationTicket} />
        <BevelLine decorative />
        <TicketTags workspaceId={data.workspaceId} {ticket} refresh={refreshOperationTicket} />
      {/key}

      <BevelLine decorative />
      <section class="ticket-detail-section ticket-relations">
        <div class="ticket-section-heading">
          <h2>Relations</h2>
          <span>{ticket.relations.outgoing.length + ticket.relations.incoming.length}</span>
        </div>
        {#if ticket.relations.blockers.length > 0}
          <div class="ticket-blocker-list">
            {#each ticket.relations.blockers as blocker}
              {#if blocker.blocking_resource_key}
                <a href={`/w/${encodeURIComponent(data.workspaceId)}/tickets/${encodeURIComponent(blocker.blocking_resource_key)}`}>
                  <strong>Dependency {blocker.blocking_resource_key} remains unresolved</strong>
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

      {#if mergeRequests.length > 0}
      <BevelLine decorative />
      <details class="ticket-control-card">
        <summary>MR requirement evidence</summary>
        <p><strong>Current requirements:</strong> {ticket.evidence.complete_for_integration ? "satisfied for integration" : "not satisfied for integration"}.</p>
        <p><strong>Current requirement approval:</strong> {currentRequirementApprovalStatus(ticket.evidence)}.</p>
        {#if ticket.evidence.missing.length > 0}
          <ul>
            {#each ticket.evidence.missing as missing}<li>{missing}</li>{/each}
          </ul>
        {/if}
        <p class="workspace-empty-copy">This evaluates linked MR requirements, separately from the Ticket case judgment and persisted integration approval.</p>
        {#if ticket.state === "closed"}
          <p class="workspace-empty-copy">This case remains closed. Incomplete MR evidence does not alter the recorded case judgment.</p>
        {/if}
      </details>
      {/if}
      <BevelLine decorative />
      <section class="ticket-control-card">
        <header><h2>Merge Requests</h2></header>
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
              <a
                class="workspace-secondary-button"
                href={mergeRequestPagePath(data.workspaceId, mergeRequest.merge_request_id)}
              >Open Merge Request</a>
            </article>
          {/each}
        {:else}
          <p class="workspace-empty-copy">No Merge Requests linked.</p>
        {/if}
      </section>

      <BevelLine decorative />
      {#key JSON.stringify([data.workspaceId, ticket.id])}
        <TicketWorkerObservation workspaceId={data.workspaceId} {ticket} />
      {/key}
      <BevelLine decorative />
      <details class="ticket-control-card ticket-worker-card">
        <summary>Role assignments and manual Workers</summary>
        <p class="workspace-empty-copy">Assignments are retained independently of Worker state.</p>
        {#if ticket.assignments.length > 0}
          <ul class="ticket-assignment-list">
            {#each ticket.assignments as assignment}
              <li>
                <strong>{assignment.role}</strong>
                <span>{assignmentPrincipalLabel(assignment.principal)}</span>
              </li>
            {/each}
          </ul>
        {:else}
          <p class="workspace-empty-copy">No role responsibility recorded.</p>
        {/if}
        {#if implementationStartEligible}
          <a class="workspace-secondary-button" href={ticketWorkerLaunchHref(data.workspaceId, ticket)}>Start a Ticket Worker</a>
          <details><summary>Assign an existing Worker</summary>
          <form class="ticket-control-form" onsubmit={assignExistingWorker}>
            <label>Runtime ID<input bind:value={manualRuntimeId} required /></label>
            <label>Worker ID<input bind:value={manualWorkerId} required /></label>
            <p class="workspace-empty-copy">Only selected Workdirs are bound. Rebinding requires authorized connection IDs.</p>
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
        {#if workerAssignment}
          <details class="ticket-cancel-implementation">
            <summary>Cancel implementation</summary>
            <form class="ticket-control-form" onsubmit={cancelImplementation}>
              <p class="workspace-empty-copy">
                Cancel the assigned Worker’s unfinished work without changing the case state. Responsibility is retained. This does not delete the Worker or Workdir.
              </p>
              <label>Reason<textarea bind:value={cancellationReason} rows="3" required></textarea></label>
              <button
                class="workspace-danger-button"
                type="submit"
                disabled={busy !== null || !cancellationReason.trim()}
              >
                {busy === "cancel-implementation" ? "Cancelling…" : "Cancel unfinished work"}
              </button>
            </form>
          </details>
        {/if}
        {#if ticket.assignment_diagnostics.length > 0}
          {#each ticket.assignment_diagnostics as diagnostic}
            <p class="workspace-callout">{diagnostic}</p>
          {/each}
        {/if}
      </details>

      <BevelLine decorative />
      <section class="ticket-control-card ticket-metadata" aria-label="Ticket metadata">
        <dl>
          <dt>Created by</dt><dd>{ticket.created_by ?? "Unknown author"}</dd>
          <dt>Created</dt><dd><time datetime={ticket.created_at ?? undefined}>{prettyDate(ticket.created_at)}</time></dd>
          <dt>Updated</dt><dd><time datetime={ticket.updated_at ?? undefined}>{prettyDate(ticket.updated_at)}</time></dd>
        </dl>
        <button class="workspace-secondary-button" type="button" onclick={() => { editTitle = ticket.title; editing = !editing; }}>
          {editing ? "Cancel title edit" : "Edit title"}
        </button>
        {#if editing}
          <form class="ticket-editor" aria-label="Edit ticket title" onsubmit={saveEdit}>
            <label>Title<input bind:value={editTitle} required disabled={busy !== null} /></label>
            <button class="workspace-primary-button" type="submit" disabled={busy !== null || !editTitle.trim()}>
              {busy === "edit" ? "Saving…" : "Save title"}
            </button>
          </form>
        {/if}
      </section>

      {#if ticket.current_closure}
        <BevelLine decorative />
        <section class="ticket-control-card"><header><h2>Current closure</h2></header>
          <p>{closureReasonLabel(ticket.current_closure.reason)} · {ticket.current_closure.author} · {prettyDate(ticket.current_closure.at)}</p>
          <RichMarkdown text={ticket.current_closure.explanation} />
          {#each ticket.current_closure.references as reference}<p>{reference.kind}: {reference.target}</p>{/each}
        </section>
      {/if}
      {#if ticket.closure_history.length > 0}
        <BevelLine decorative />
        <details class="ticket-control-card"><summary>Case history ({ticket.closure_history.length})</summary>
          {#each ticket.closure_history as judgment (judgment.operation_key)}
            <article>
              <p>{relationLabel(judgment.action.action)}{judgment.action.action !== "reopen" ? `: ${closureReasonLabel(judgment.action.reason)}` : ""} · {judgment.author} · {prettyDate(judgment.at)}</p>
              <RichMarkdown text={judgment.explanation} />
              {#each judgment.references as reference}<p>{reference.kind}: {reference.target}</p>{/each}
            </article>
          {/each}
        </details>
      {/if}
      {#if ticket.legacy_handoff}
        <BevelLine decorative />
        <details class="ticket-control-card"><summary>Legacy routing handoff</summary>
          <p>Historical state: {ticket.legacy_handoff.workflow_state}</p>
          {#if ticket.legacy_handoff.resolution}<RichMarkdown text={ticket.legacy_handoff.resolution} />{/if}
        </details>
      {/if}
    </aside>
  </div>
</div>
