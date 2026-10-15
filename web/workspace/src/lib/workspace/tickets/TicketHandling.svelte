<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import BevelLine from "#lib/workspace/ui/BevelLine.svelte";
  import type { TicketDetail, TicketAssignmentTrackingProjection, TicketAssignmentTrackingHistoryPage, TicketAssignmentsPage, TicketAssignmentPrincipalSnapshot } from "#lib/generated/ticket-api.ts";
  import type { WorkspaceOrchestratorStatus } from "./ticket-panel.ts";
  import { workspaceApiJson, workspaceApiJsonWithBody, formatDate } from "#lib/workspace/api/http.ts";
  import { parseTicketRoleAssignmentMutationResponse, TICKET_BROWSER_API_MAX_RESPONSE_BYTES } from "#lib/workspace/api/ticket-browser.ts";
  import { assignmentOperation, answerOperation, parseAnswerReceipt, parseAssignmentsPage, operationError, parseTracking, parseTrackingHistory, trackingPath, type ObservedOperation } from "#lib/workspace/api/ticket-operations.ts";
  let { workspaceId, ticket, orchestrator, refresh }: { workspaceId: string; ticket: TicketDetail; orchestrator: WorkspaceOrchestratorStatus | null; refresh: () => Promise<void> } = $props();
  const current = $derived(ticket.assignment_tracking.find((row) => row.current && row.role === "orchestrator"));
  let busy = $state(false);
  let pending = $state<ObservedOperation | null>(null);
  let error = $state<string | null>(null);
  let savedNotice = $state<string | null>(null);
  let pendingKind = $state<"assignment" | "answer">("assignment");
  let assignments = $state<TicketAssignmentsPage | null>(null);
  let assignmentPageBusy = $state(false);
  let answer = $state("");
  let principalKind = $state<"workspace_agent" | "user" | "worker">("workspace_agent");
  let accountId = $state("");
  let runtimeId = $state("");
  let workerId = $state("");
  const principalReady = $derived(principalKind === "workspace_agent" || (principalKind === "user" ? !!accountId.trim() : !!runtimeId.trim() && !!workerId.trim()));
  function selectedPrincipal(): TicketAssignmentPrincipalSnapshot {
    if (principalKind === "user") return { kind: "user", account_id: accountId.trim() };
    if (principalKind === "worker") return { kind: "worker", runtime_id: runtimeId.trim(), worker_id: workerId.trim() };
    return { kind: "workspace_agent", agent_key: "workspace-orchestrator" };
  }
  let historyId = $state("");
  let retained = $state<TicketAssignmentTrackingProjection | null>(null);
  let history = $state<TicketAssignmentTrackingHistoryPage | null>(null);
  let historyError = $state<string | null>(null);
  let historyBusy = $state(false);
  let alive = true;
  onDestroy(() => { alive = false; });
  async function run(operation: ObservedOperation, kind: "assignment" | "answer" = pendingKind) {
    if (busy) return;
    busy = true; error = null; pending = operation; pendingKind = kind; savedNotice = null;
    try {
      if (kind === "answer") {
        const receipt = await workspaceApiJsonWithBody(operation.path, { method: operation.method, body: operation.body }, parseAnswerReceipt, TICKET_BROWSER_API_MAX_RESPONSE_BYTES);
        if (!alive) return;
        savedNotice = `Answer saved to assignment ${receipt.assignment_id}, input ${receipt.input_event_id}. Handling status is unchanged.`;
        answer = "";
      } else {
        const result = await workspaceApiJsonWithBody(operation.path, { method: operation.method, body: operation.body }, parseTicketRoleAssignmentMutationResponse, TICKET_BROWSER_API_MAX_RESPONSE_BYTES);
        if (!alive) return;
        savedNotice = `Assignment operation saved: ${result.assignment?.assignment_id ?? "released"}.`;
      }
      pending = null;
      await refresh();
      await readAssignments();
    } catch (cause) { if (alive) error = savedNotice ? `Operation saved; latest projection unavailable. ${operationError(cause)}` : operationError(cause); }
    finally { busy = false; }
  }
  function delegate(clear = false) {
    if (!clear && !principalReady) return;
    void run(assignmentOperation(workspaceId, ticket.id, ticket, crypto.randomUUID(), clear, selectedPrincipal()), "assignment");
  }
  async function reconcile() {
    if (busy) return;
    busy = true;
    try { await refresh(); if (!alive) return; pending = null; error = null; savedNotice = null; await readAssignments(); }
    catch (cause) { if (alive) error = operationError(cause); }
    finally { busy = false; }
  }
  async function readHistory(assignmentId: string, more = false) {
    if (!assignmentId || historyBusy) return;
    historyBusy = true; historyError = null;
    const root = trackingPath(workspaceId, ticket.id);
    try {
      const rows = await workspaceApiJson(`${root}?${new URLSearchParams({ assignment_id: assignmentId })}`, parseTracking, TICKET_BROWSER_API_MAX_RESPONSE_BYTES);
      retained = rows[0] ?? null;
      const search = new URLSearchParams({ assignment_id: assignmentId, limit: "20" });
      if (more && history?.next_event_id) search.set("after_event_id", history.next_event_id);
      const page = await workspaceApiJson(`${root}/history?${search}`, parseTrackingHistory, TICKET_BROWSER_API_MAX_RESPONSE_BYTES);
      history = more && history ? { ...page, items: [...history.items, ...page.items] } : page;
      historyId = assignmentId;
    } catch (cause) { historyError = operationError(cause); }
    finally { historyBusy = false; }
  }

  async function readAssignments(more = false) {
    if (assignmentPageBusy) return;
    assignmentPageBusy = true;
    try {
      const query = new URLSearchParams({ limit: "20" });
      if (more && assignments?.next_assignment_id) query.set("after_assignment_id", assignments.next_assignment_id);
      const page = await workspaceApiJson(trackingPath(workspaceId, ticket.id) + "/assignments?" + query, parseAssignmentsPage, TICKET_BROWSER_API_MAX_RESPONSE_BYTES);
      if (!alive) return;
      assignments = more && assignments ? { ...page, items: [...assignments.items, ...page.items] } : page;
    } catch (cause) { historyError = operationError(cause); }
    finally { assignmentPageBusy = false; }
  }
  function submitAnswer(event: SubmitEvent) {
    event.preventDefault();
    if (!current || pending || !answer.trim()) return;
    void run(answerOperation(workspaceId, ticket.id, current, answer, crypto.randomUUID()), "answer");
  }
  onMount(() => { void readAssignments(); });
</script>
<section class="ticket-detail-section ticket-handling">
  <div class="ticket-section-heading"><h2>Orchestrator handling</h2><a href={`/w/${encodeURIComponent(workspaceId)}/orchestrator`}>All handling</a></div>
  {#if current}
    <p><strong>Self-reported handling: {current.tracking?.status ?? "unknown"}</strong></p>
    {#if current.tracking}
      <dl>
        {#if current.tracking.waiting_reason}<dt>Waiting reason</dt><dd>{current.tracking.waiting_reason}</dd>{/if}
        {#if current.tracking.progress}<dt>Progress</dt><dd>{current.tracking.progress}</dd>{/if}
        {#if current.tracking.next_action}<dt>Next action</dt><dd>{current.tracking.next_action}</dd>{/if}
        {#if current.tracking.result}<dt>Result of this handling</dt><dd>{current.tracking.result}</dd>{/if}
      </dl>
    {/if}
    {#if current.updated_at}<p>Updated {formatDate(current.updated_at)}</p>{/if}
    {#if current.migration_diagnostic}<p class="workspace-callout">{current.migration_diagnostic}</p>{/if}
    {#if current.tracking?.status === "done"}<p>This handling was returned. Ticket {ticket.state} is unchanged.</p>{/if}
    <details class="ticket-assignment-details"><summary>Assignment details</summary>
    <p>Principal: {current.principal.kind === "workspace_agent" ? current.principal.agent_key : current.principal.kind === "worker" ? `${current.principal.runtime_id}/${current.principal.worker_id}` : current.principal.account_id}</p>
    <p>Assignment <code>{current.assignment_id}</code> · assigned by {current.assigned_by} · {formatDate(current.assigned_at)}</p>
    </details>
  {:else}<p class="workspace-empty-copy">No current Orchestrator assignment.</p>{/if}
  {#if error}<p class="workspace-callout is-error" role="alert">{error}</p>{/if}
  {#if savedNotice}<p role="status">{savedNotice}</p>{/if}
  {#if pending}
    <p>Unresolved operation. New delegation is disabled until reconciled.</p>
    <button class="workspace-secondary-button" disabled={busy} onclick={() => pending && void run(pending)}>Retry exact operation</button>
    <button class="workspace-secondary-button" disabled={busy} onclick={() => void reconcile()}>Refresh and reconcile assignment</button>
  {/if}
  {#if ticket.state === "closed" && ticket.action_eligibility.can_assign_orchestrator}<p class="workspace-empty-copy">A delegation can be saved, but this closed case does not admit new work. Reopening the case is a separate decision and does not itself reissue handling.</p>{/if}
  {#if ticket.action_eligibility.can_assign_orchestrator}
    <details class="assignment-principal"><summary>Choose responsibility principal</summary>
      <fieldset class="ticket-control-form" disabled={busy || pending !== null}>
        <label>Principal kind<select bind:value={principalKind}><option value="workspace_agent">Workspace Orchestrator agent</option><option value="user">User account</option><option value="worker">Worker principal</option></select></label>
        {#if principalKind === "user"}<label>Account identity<input bind:value={accountId} maxlength="1024" /></label>{/if}
        {#if principalKind === "worker"}<label>Runtime identity<input bind:value={runtimeId} maxlength="1024" /></label><label>Worker identity<input bind:value={workerId} maxlength="1024" /></label>{/if}
      </fieldset>
      <p>Backend validates the selected principal. Responsibility does not choose or start a concrete Worker.</p>
    </details>
  {/if}
  <div class="handling-actions">
    {#if ticket.action_eligibility.can_assign_orchestrator}
      {#if !current || current.tracking?.status === "done"}
        <button class="workspace-primary-button" disabled={busy || pending !== null || !principalReady} onclick={() => delegate()}>{busy ? "Saving…" : current ? "Request again (new assignment)" : "Request Orchestrator"}</button>
      {:else}
        <details><summary>Replace this delegation</summary><p>A replacement starts a new assignment even for the same principal. The previous result and release history remain.</p><button class="workspace-secondary-button" disabled={busy || pending !== null || !principalReady} onclick={() => { if (confirm("Replace this delegation with a new assignment to the selected principal?")) delegate(); }}>Replace assignment</button></details>
      {/if}
    {/if}
    {#if ticket.action_eligibility.can_unassign_orchestrator}<button class="workspace-danger-button" disabled={busy || pending !== null} onclick={() => { if (confirm("Release responsibility? This does not stop any running Worker.")) delegate(true); }}>Release assignment</button>{/if}
  </div>
  {#if ticket.state === "open" && current?.last_event_id && current.tracking && current.tracking.status !== "done"}
    <form class="ticket-control-form ticket-answer" onsubmit={submitAnswer}>
      <label>Answer / continue this handling<textarea bind:value={answer} rows="3" required disabled={busy || pending !== null}></textarea></label>
      <button class="workspace-secondary-button" disabled={busy || pending !== null || !answer.trim()}>Send answer to this assignment</button>
    </form>
  {/if}
  <details class="ticket-assignment-history"><summary>Past assignment and release history</summary>
    {#if current}<button class="workspace-secondary-button" disabled={historyBusy} onclick={() => void readHistory(current.assignment_id)}>View handling history</button>{/if}
    {#if assignmentPageBusy}<p role="status">Loading assignment history…</p>{/if}
    {#each assignments?.items ?? [] as row (row.assignment_id)}<p><button class="workspace-secondary-button" disabled={historyBusy} onclick={() => void readHistory(row.assignment_id)}>{row.current ? "Current" : "Past"} · {row.tracking?.status ?? "unknown"} · {formatDate(row.assigned_at)}</button> <code>{row.assignment_id}</code>{#if row.release} · {row.release.action}{/if}{#if row.tracking?.result} · {row.tracking.result}{/if}</p>{/each}
    {#if assignments?.next_assignment_id}<button class="workspace-secondary-button" disabled={assignmentPageBusy} onclick={() => void readAssignments(true)}>More assignments</button>{/if}
    <form onsubmit={(event) => { event.preventDefault(); void readHistory(historyId); }}><label>Assignment identity<input bind:value={historyId} required /></label><button class="workspace-secondary-button" disabled={historyBusy}>Read assignment</button></form>
    {#if historyError}<p role="alert" class="workspace-callout is-error">{historyError}</p>{/if}
    {#if retained}
      <p><code>{retained.assignment_id}</code> · {retained.current ? "current" : "past"} · last handling {retained.tracking?.status ?? "unknown"}</p>
      {#if retained.tracking?.result}<p>Previous result: {retained.tracking.result}</p>{/if}
      {#if retained.release}<p>{retained.release.action} by {retained.release.actor} · {formatDate(retained.release.at)}</p>{#if retained.release.replacement_assignment_id}<button class="workspace-secondary-button" disabled={historyBusy} onclick={() => void readHistory(retained!.release!.replacement_assignment_id!)}>Read replacement assignment</button>{/if}{/if}
    {/if}
    {#if history}{#each history.items as entry (entry.event_id)}<article><p>{entry.tracking.status} · {formatDate(entry.at)}</p>{#if entry.tracking.waiting_reason}<p>Waiting reason: {entry.tracking.waiting_reason}</p>{/if}{#if entry.tracking.progress}<p>Progress: {entry.tracking.progress}</p>{/if}{#if entry.tracking.next_action}<p>Next action: {entry.tracking.next_action}</p>{/if}{#if entry.tracking.result}<p>Result: {entry.tracking.result}</p>{/if}</article>{/each}{#if history.next_event_id}<button class="workspace-secondary-button" disabled={historyBusy} onclick={() => void readHistory(historyId, true)}>More handling history</button>{/if}{/if}
  </details>
</section>
<BevelLine decorative />
<details class="ticket-detail-section ticket-orchestrator-worker">
  <summary>Orchestrator Worker</summary>
  {#if orchestrator?.worker}<p>{orchestrator.worker.runtime_id}/{orchestrator.worker.worker_id} · actual state: <strong>{orchestrator.worker.state}</strong> · {orchestrator.online ? "online" : "offline"}</p><a href={`/w/${encodeURIComponent(workspaceId)}/workers/${encodeURIComponent(orchestrator.worker.worker_id)}?runtimeId=${encodeURIComponent(orchestrator.worker.runtime_id)}`}>Open Worker</a>{:else}<p>Worker observation unavailable. This does not change handling status.</p>{/if}
</details>
<style>
  .assignment-principal fieldset { min-width: 0; margin: var(--space-3, 0.75rem) 0 0; padding: 0; border: 0; }
  dl { display: grid; grid-template-columns: minmax(0, 1fr); gap: var(--space-1); }
  dd + dt { margin-top: var(--space-2); }
  dt { color: var(--text-muted); } dd { margin: 0; white-space: pre-wrap; overflow-wrap: anywhere; }
  code { overflow-wrap: anywhere; } .handling-actions { display: flex; gap: 0.5rem; flex-wrap: wrap; margin-block: 1rem; }
  article + article { margin-top: 1rem; }

</style>
