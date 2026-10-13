<script lang="ts">
  import { untrack, onDestroy } from "svelte";
  import type { TicketDetail } from "#lib/generated/ticket-api.ts";
  import type { WorkerSummary } from "#lib/generated/worker-launch-api.ts";
  import { workspaceApiJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
  import { parseWorkerSummary } from "#lib/workspace/api/workers.ts";
  import { workerHref } from "#lib/workspace/resource-links.ts";
  let { workspaceId, ticket }: { workspaceId: string; ticket: TicketDetail } = $props();
  let worker = $state<WorkerSummary | null>(null);
  let error = $state<string | null>(null);
  let loading = $state(false);
  let observationRequest = 0;
  onDestroy(() => { observationRequest += 1; });
  async function load() {
    const request = ++observationRequest;
    const reference = ticket.current_worker?.worker_resource_key;
    worker = null; error = null;
    if (!reference) { loading = false; return; }
    loading = true;
    try {
      const observed = await workspaceApiJson(workspaceApiPath(workspaceId, `/workers/${encodeURIComponent(reference)}`), parseWorkerSummary, 1024 * 1024);
      if (request !== observationRequest) return;
      worker = observed;
    }
    catch (cause) { if (request === observationRequest) error = cause instanceof Error ? cause.message : String(cause); }
    finally { if (request === observationRequest) loading = false; }
  }
  $effect(() => { void ticket.current_worker?.worker_resource_key; untrack(() => { void load(); }); });
</script>
<details class="ticket-detail-section ticket-worker-observation">
  <summary>Related Worker — actual state</summary>
  <button class="workspace-secondary-button" disabled={loading} onclick={() => void load()}>Refresh Worker observation</button>
  {#if loading}<p role="status">Loading Worker observation…</p>{/if}
  {#if error}<p role="alert" class="workspace-callout is-error">Worker observation unavailable: {error}</p>{/if}
  {#if worker}<p>{worker.runtime_id}/{worker.worker_id}: <strong>{worker.state}</strong> · availability {worker.availability}</p><a href={workerHref(workspaceId, { resource_key: worker.resource_key, display_name: worker.display_name ?? worker.label })}>Open Worker</a>
  {:else}<p class="workspace-empty-copy">{ticket.current_worker ? "Retained Worker identity; live state not observed." : "No related Worker recorded."}</p>{/if}
  <p class="workspace-empty-copy">Worker state does not determine Ticket case state or self-reported handling.</p>
</details>
