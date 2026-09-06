<script lang="ts">
  import type {
    Diagnostic,
    WorkspaceDeletionOperationResponse,
    WorkspaceDeletionPreflightResponse,
    WorkspaceDeletionRequest,
    WorkspaceMetadataSettingsResponse,
  } from '$lib/generated/workspace-api';
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  import { disposeWorkspaceMultiplexer } from '$lib/workspace/multiplexer';
  import { disposeWorkspaceWorkersStore } from '$lib/workspace/sidebar/worker-subscription';
  import {
    getWorkspaceDeletion,
    preflightWorkspaceDeletion,
    startWorkspaceDeletion,
  } from '$lib/workspace/settings/workspace-deletion-api';
  import DiagnosticsList from '$lib/workspace/settings/DiagnosticsList.svelte';
  import {
    fetchWorkspaceMetadata,
    updateWorkspaceMetadata,
  } from '$lib/workspace/settings/profile-api';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
  let workspaceId = $derived(data.workspace?.workspace_id ?? '');

  let workspaceMetadata = $state<WorkspaceMetadataSettingsResponse | null>(null);
  let displayNameDraft = $state('');
  let loading = $state(true);
  let submitting = $state(false);
  let message = $state<string | null>(null);
  let diagnostics = $state<Diagnostic[]>([]);
  let deletionOpen = $state(false);
  let deletionLoading = $state(false);
  let deletionSubmitting = $state(false);
  let deletionConfirmation = $state('');
  let deletionPreflight = $state<WorkspaceDeletionPreflightResponse | null>(null);
  let deletionOperation = $state<WorkspaceDeletionOperationResponse | null>(null);
  let deletionRequest = $state<WorkspaceDeletionRequest | null>(null);
  let deletionError = $state<string | null>(null);
  function deletionStorageKey(): string {
    return `yoi:workspace-deletion:${workspaceId}`;
  }

  $effect(() => {
    if (!workspaceId) {
      loading = false;
      return;
    }
    let cancelled = false;
    async function load() {
      loading = true;
      message = null;
      try {
        const response = await fetchWorkspaceMetadata(workspaceId);
        if (!cancelled) {
          workspaceMetadata = response;
          displayNameDraft = response.display_name;
          diagnostics = response.diagnostics;
        }
      } catch (err) {
        if (!cancelled) {
          message = err instanceof Error ? err.message : 'workspace settings request failed';
        }
      } finally {
        if (!cancelled) loading = false;
      }
    }
    load();
    return () => {
      cancelled = true;
    };
  });

  async function submitWorkspaceName() {
    if (!workspaceMetadata) return;
    submitting = true;
    message = null;
    try {
      const response = await updateWorkspaceMetadata(workspaceId, {
        display_name: displayNameDraft,
        revision: workspaceMetadata.revision
      });
      workspaceMetadata = response.workspace;
      displayNameDraft = response.workspace.display_name;
      diagnostics = response.diagnostics.concat(response.workspace.diagnostics);
      message = 'Workspace display name updated.';
    } catch (err) {
      message = err instanceof Error ? err.message : 'workspace update failed';
    } finally {
      submitting = false;
    }
  }

  async function openDeletionConfirmation() {
    deletionOpen = true;
    deletionLoading = true;
    deletionError = null;
    deletionOperation = null;
    deletionRequest = null;
    sessionStorage.removeItem(deletionStorageKey());
    deletionConfirmation = '';
    try {
      deletionPreflight = await preflightWorkspaceDeletion(workspaceId);
    } catch (err) {
      deletionError = err instanceof Error ? err.message : 'Workspace deletion preflight failed';
    } finally {
      deletionLoading = false;
    }
  }

  async function trackDeletion(operationId: string) {
    let operation = await getWorkspaceDeletion(operationId);
    deletionOperation = operation;
    while (operation.state === 'queued' || operation.state === 'running') {
      await new Promise((resolve) => setTimeout(resolve, 500));
      operation = await getWorkspaceDeletion(operation.operation_id);
      deletionOperation = operation;
    }
    if (operation.state === 'succeeded') {
      sessionStorage.removeItem(deletionStorageKey());
      disposeWorkspaceMultiplexer(workspaceId);
      disposeWorkspaceWorkersStore(workspaceId);
      await goto('/');
    }
  }

  function storedDeletionRequest(): WorkspaceDeletionRequest | null {
    try {
      const value: unknown = JSON.parse(sessionStorage.getItem(deletionStorageKey()) ?? 'null');
      if (typeof value !== 'object' || value === null) return null;
      const record = value as Record<string, unknown>;
      if (
        Object.keys(record).sort().join(',') !== 'confirmation,expected_revision,operation_id' ||
        typeof record.operation_id !== 'string' || record.operation_id.length === 0 || record.operation_id.length > 128 ||
        !/^[A-Za-z0-9_-]+$/.test(record.operation_id) ||
        typeof record.expected_revision !== 'string' || record.expected_revision.length > 128 ||
        typeof record.confirmation !== 'string' || record.confirmation !== data.workspace?.display_name || record.confirmation.length > 256
      ) return null;
      return {
        operation_id: record.operation_id,
        expected_revision: record.expected_revision,
        confirmation: record.confirmation,
      };
    } catch {
      return null;
    }
  }

  onMount(() => {
    if (!data.workspace?.permissions.delete_workspace) return;
    const request = storedDeletionRequest();
    if (!request) return;
    deletionRequest = request;
    deletionConfirmation = request.confirmation;
    deletionOpen = true;
    deletionSubmitting = true;
    void trackDeletion(request.operation_id)
      .catch((err) => {
        deletionError = err instanceof Error ? err.message : 'Workspace deletion status failed';
      })
      .finally(() => {
        deletionSubmitting = false;
      });
  });

  async function deleteWorkspace() {
    if (!deletionPreflight && !deletionRequest) return;
    deletionSubmitting = true;
    deletionError = null;
    try {
      const request = deletionRequest ?? {
        operation_id: crypto.randomUUID(),
        expected_revision: deletionPreflight!.expected_revision,
        confirmation: deletionConfirmation,
      };
      deletionRequest = request;
      sessionStorage.setItem(deletionStorageKey(), JSON.stringify(request));
      const operation = await startWorkspaceDeletion(workspaceId, request);
      deletionOperation = operation;
      await trackDeletion(operation.operation_id);
    } catch (err) {
      deletionError = err instanceof Error ? err.message : 'Workspace deletion failed';
    } finally {
      deletionSubmitting = false;
    }
  }
</script>

<svelte:head>
  <title>Workspace settings · Yoi Workspace</title>
</svelte:head>

<section class="card settings-section" aria-labelledby="workspace-settings-title">
  <header class="settings-section-header">
    <div>
      <p class="eyebrow">editable</p>
      <h2 id="workspace-settings-title">Workspace Identity</h2>
    </div>
    <span class="badge success">Backend scoped</span>
  </header>

  {#if loading}
    <p class="status-message">Loading workspace settings…</p>
  {:else}
    <form class="settings-form" onsubmit={(event) => { event.preventDefault(); void submitWorkspaceName(); }}>
      <label>
        <span>Display name</span>
        <input bind:value={displayNameDraft} autocomplete="off" />
      </label>
      <p class="settings-note">Workspace id: <code>{workspaceMetadata?.workspace_id ?? workspaceId}</code></p>
      <button type="submit" disabled={submitting || !workspaceMetadata}>{submitting ? 'Saving…' : 'Save workspace name'}</button>
    </form>

    <dl class="settings-identity-list">
      <div>
        <dt>Source</dt>
        <dd>{workspaceMetadata?.source ?? 'unknown'}</dd>
      </div>
      <div>
        <dt>Revision</dt>
        <dd><code>{workspaceMetadata?.revision ?? 'unknown'}</code></dd>
      </div>
    </dl>
  {/if}

  {#if message}
    <p class="status-message" class:error={message.includes('failed')}>{message}</p>
  {/if}
  <DiagnosticsList {diagnostics} />
</section>

{#if data.workspace?.permissions.delete_workspace}
  <section class="settings-section danger-zone" aria-labelledby="workspace-danger-title">
    <div>
      <h2 id="workspace-danger-title">Danger zone</h2>
      <p>Deleting this Workspace permanently removes its Workers, Workdirs, repositories, configuration, Memory, Tickets, and audit data.</p>
    </div>
    <button class="danger-button" type="button" onclick={() => void openDeletionConfirmation()}>Delete Workspace</button>
  </section>
{/if}

{#if deletionOpen}
  <div class="modal-backdrop" role="presentation">
    <div class="deletion-dialog" role="dialog" aria-modal="true" aria-labelledby="delete-workspace-title">
      <h2 id="delete-workspace-title">Delete {deletionPreflight?.display_name ?? deletionRequest?.confirmation ?? 'Workspace'}?</h2>
      {#if deletionLoading}
        <p>Loading deletion impact…</p>
      {:else if deletionPreflight}
        <p>This operation cannot be undone. It will remove:</p>
        <ul>
          <li>{deletionPreflight.resources.workers} Workers</li>
          <li>{deletionPreflight.resources.workdirs} Workdirs</li>
          <li>{deletionPreflight.resources.repositories} repositories</li>
          <li>{deletionPreflight.resources.runtime_bindings} Runtime bindings</li>
          <li>{deletionPreflight.resources.secrets} secret records</li>
          <li>{deletionPreflight.resources.artifacts} artifacts</li>
        </ul>
        {#each deletionPreflight.blockers as blocker}
          <p class="status-message error">{blocker.message}</p>
        {/each}
        <label>
          <span>Type <strong>{deletionPreflight.display_name}</strong> to confirm</span>
          <input bind:value={deletionConfirmation} autocomplete="off" />
        </label>
      {/if}
      {#if deletionOperation}
        <p class="status-message">Deletion state: {deletionOperation.state}</p>
        {#each deletionOperation.blockers as blocker}
          <p class="status-message error">{blocker.message}</p>
        {/each}
      {/if}
      {#if deletionError}<p class="status-message error">{deletionError}</p>{/if}
      <div class="dialog-actions">
        <button type="button" onclick={() => { deletionOpen = false; }} disabled={deletionSubmitting}>Cancel</button>
        <button
          class="danger-button"
          type="button"
          onclick={() => void deleteWorkspace()}
          disabled={deletionSubmitting || (!deletionRequest && !deletionPreflight?.can_delete) || deletionConfirmation !== (deletionPreflight?.display_name ?? deletionRequest?.confirmation ?? '')}
        >{deletionSubmitting ? 'Deleting…' : 'Delete Workspace'}</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .danger-zone { display: flex; justify-content: space-between; align-items: start; gap: var(--space-4); border-top: 1px solid var(--color-danger, #b42318); }
  .danger-zone p { max-width: 68ch; }
  .danger-button { color: white; background: var(--color-danger, #b42318); border-color: var(--color-danger, #b42318); }
  .modal-backdrop { position: fixed; inset: 0; z-index: 100; display: grid; place-items: center; padding: var(--space-4); background: rgb(0 0 0 / 0.55); }
  .deletion-dialog { width: min(34rem, 100%); max-height: calc(100vh - 2rem); overflow: auto; padding: var(--space-5); background: var(--color-surface, white); border: 1px solid var(--color-border); }
  .deletion-dialog label { display: grid; gap: var(--space-2); margin-block: var(--space-4); }
  .dialog-actions { display: flex; justify-content: flex-end; gap: var(--space-2); margin-top: var(--space-5); }
</style>
