<script lang="ts">
  import { invalidate, goto } from '$app/navigation';
  import { onMount, onDestroy, tick } from 'svelte';
  import type {
    Diagnostic, WorkspaceMetadataSettingsResponse, WorkspaceSigningIdentityResponse,
    WorkspaceDeletionPreflightResponse, WorkspaceDeletionOperationResponse, WorkspaceDeletionRequest,
  } from '#lib/generated/legacy-server-api.ts';
  import { workspaceApiPath } from '#lib/workspace/api/http.ts';
  import { disposeWorkspaceMultiplexer } from '#lib/workspace/multiplexer.ts';
  import { disposeWorkspaceWorkersStore } from '#lib/workspace/sidebar/worker-subscription.ts';
  import { fetchWorkspaceMetadata, updateWorkspaceMetadata, fetchWorkspaceSigningIdentity, provisionWorkspaceSigningIdentity } from './profile-api';
  import { preflightWorkspaceDeletion, startWorkspaceDeletion, getWorkspaceDeletion } from './workspace-deletion-api';
  import BevelLine from '#lib/workspace/ui/BevelLine.svelte';

  // The route keys this component by Workspace and owner permission. Requests from
  // a disposed scope must not publish state, focus, or navigation into a new one.
  let { workspaceId, workspaceName, canDelete }: { workspaceId: string; workspaceName: string; canDelete: boolean } = $props();
  let disposed = false;
  onDestroy(() => { disposed = true; });
  let workspaceMetadata = $state<WorkspaceMetadataSettingsResponse | null>(null);
  let loading = $state(true);
  let metadataError = $state<string | null>(null);
  let diagnostics = $state<Diagnostic[]>([]);
  let editing = $state(false);
  let displayNameDraft = $state('');
  let submitting = $state(false);
  let nameError = $state<string | null>(null);
  let nameNotice = $state<string | null>(null);
  let editButton = $state<HTMLButtonElement>();
  let identityCopyButton = $state<HTMLButtonElement>();
  let provisionButton = $state<HTMLButtonElement>();

  let signingIdentity = $state<WorkspaceSigningIdentityResponse | null>(null);
  let identityLoading = $state(true);
  let identityError = $state<string | null>(null);
  let provisioningIdentity = $state(false);
  let copyError = $state<string | null>(null);
  let identityCopied = $state(false);
  let identityBundleText = $derived(signingIdentity?.public_bundle ? JSON.stringify(signingIdentity.public_bundle, null, 2) : '');

  let dangerOpen = $state(false);
  let dangerSummary = $state<HTMLElement>();
  let deletionOpen = $state(false);
  let deletionLoading = $state(false);
  let deletionSubmitting = $state(false);
  let deletionConfirmation = $state('');
  let deletionPreflight = $state<WorkspaceDeletionPreflightResponse | null>(null);
  let deletionOperation = $state<WorkspaceDeletionOperationResponse | null>(null);
  let deletionRequest = $state<WorkspaceDeletionRequest | null>(null);
  let deletionError = $state<string | null>(null);
  const errorText = (error: unknown, fallback: string) => error instanceof Error ? error.message : fallback;
  const focusInput = (element: HTMLElement) => { element.focus(); };
  const deletionStorageKey = () => `yoi:workspace-deletion:${workspaceId}`;

  async function loadMetadata() {
    loading = true;
    metadataError = null;
    try {
      const response = await fetchWorkspaceMetadata(workspaceId);
      if (disposed) return;
      workspaceMetadata = response;
      displayNameDraft = response.display_name;
      diagnostics = response.diagnostics;
    } catch (error) {
      if (!disposed) metadataError = errorText(error, 'Workspace settings unavailable.');
    } finally {
      if (!disposed) loading = false;
    }
  }
  async function loadIdentity() {
    if (!canDelete) return;
    identityLoading = true;
    identityError = null;
    try {
      const response = await fetchWorkspaceSigningIdentity(workspaceId);
      if (!disposed) signingIdentity = response;
    } catch (error) {
      if (!disposed) identityError = errorText(error, 'Public identity unavailable.');
    } finally {
      if (!disposed) identityLoading = false;
    }
  }
  async function retryMetadata() {
    await loadMetadata();
    await tick();
    if (!disposed && !metadataError) editButton?.focus();
  }
  async function retryIdentity() {
    await loadIdentity();
    await tick();
    if (!disposed && !identityError) (identityCopyButton ?? provisionButton)?.focus();
  }
  function editName() {
    if (!workspaceMetadata) return;
    displayNameDraft = workspaceMetadata.display_name;
    nameError = nameNotice = null;
    editing = true;
  }
  async function cancelName() {
    editing = false;
    nameError = null;
    displayNameDraft = workspaceMetadata?.display_name ?? '';
    await tick();
    if (!disposed) editButton?.focus();
  }
  async function reloadName() {
    await loadMetadata();
    if (!disposed && !metadataError) await cancelName();
  }
  async function submitWorkspaceName() {
    if (!workspaceMetadata || submitting || !displayNameDraft.trim()) return;
    submitting = true;
    nameError = nameNotice = null;
    try {
      const response = await updateWorkspaceMetadata(workspaceId, {
        display_name: displayNameDraft, expected_updated_at: workspaceMetadata.updated_at,
      });
      if (disposed) return;
      workspaceMetadata = response.workspace;
      diagnostics = response.diagnostics.concat(response.workspace.diagnostics);
      nameNotice = 'Workspace name saved.';
      await cancelName();
      if (disposed) return;
      try { await invalidate(workspaceApiPath(workspaceId, '/workspace')); }
      catch { if (!disposed) nameNotice = 'Workspace name saved. Reload the page to update navigation.'; }
    } catch (error) {
      if (!disposed) nameError = errorText(error, 'Workspace name could not be saved.');
    } finally {
      if (!disposed) submitting = false;
    }
  }
  async function provisionIdentity() {
    if (!canDelete || provisioningIdentity) return;
    provisioningIdentity = true;
    identityError = null;
    try {
      const response = await provisionWorkspaceSigningIdentity(workspaceId);
      if (disposed) return;
      signingIdentity = response;
      await tick();
      if (!disposed) identityCopyButton?.focus();
    } catch (error) {
      if (!disposed) identityError = errorText(error, 'Identity provisioning failed.');
    } finally {
      if (!disposed) provisioningIdentity = false;
    }
  }
  async function copyIdentityBundle() {
    if (!identityBundleText) return;
    copyError = null;
    identityCopied = false;
    try {
      await navigator.clipboard.writeText(identityBundleText);
      if (!disposed) identityCopied = true;
    } catch {
      if (!disposed) copyError = 'Could not copy. Open Public bundle and key details to select the bundle manually.';
    }
  }

  async function openDeletionConfirmation() {
    if (!canDelete || editing || submitting || provisioningIdentity || deletionLoading || deletionSubmitting) return;
    deletionOpen = true;
    deletionLoading = true;
    deletionError = null;
    deletionPreflight = deletionOperation = deletionRequest = null;
    deletionConfirmation = '';
    try {
      sessionStorage.removeItem(deletionStorageKey());
      const response = await preflightWorkspaceDeletion(workspaceId);
      if (!disposed) deletionPreflight = response;
    } catch (error) {
      if (!disposed) deletionError = errorText(error, 'Workspace deletion preflight failed.');
    } finally {
      if (!disposed) deletionLoading = false;
    }
  }
  async function cancelDeletion() {
    deletionOpen = dangerOpen = false;
    await tick();
    if (!disposed) dangerSummary?.focus();
  }
  async function trackDeletion(operationId: string) {
    let operation = await getWorkspaceDeletion(operationId);
    if (disposed) return;
    deletionOperation = operation;
    while (operation.state === 'queued' || operation.state === 'running') {
      await new Promise((resolve) => setTimeout(resolve, 500));
      if (disposed) return;
      operation = await getWorkspaceDeletion(operation.operation_id);
      if (disposed) return;
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
        Object.keys(record).sort().join(',') !== 'confirmation,expected_workspace_updated_at,operation_id' ||
        typeof record.operation_id !== 'string' || record.operation_id.length === 0 || record.operation_id.length > 128 ||
        !/^[A-Za-z0-9_-]+$/.test(record.operation_id) ||
        typeof record.expected_workspace_updated_at !== 'string' || record.expected_workspace_updated_at.length > 128 ||
        typeof record.confirmation !== 'string' || record.confirmation !== workspaceName || record.confirmation.length > 256
      ) return null;
      return { operation_id: record.operation_id, expected_workspace_updated_at: record.expected_workspace_updated_at, confirmation: record.confirmation };
    } catch { return null; }
  }
  onMount(() => {
    void loadMetadata();
    void loadIdentity();
    if (!canDelete) return;
    const request = storedDeletionRequest();
    if (!request) return;
    deletionRequest = request;
    deletionConfirmation = request.confirmation;
    dangerOpen = deletionOpen = deletionSubmitting = true;
    void trackDeletion(request.operation_id)
      .catch((error) => { if (!disposed) deletionError = errorText(error, 'Workspace deletion status failed.'); })
      .finally(() => { if (!disposed) deletionSubmitting = false; });
  });
  async function deleteWorkspace() {
    if (!canDelete || deletionSubmitting || (!deletionPreflight && !deletionRequest)) return;
    if (!deletionRequest && (!deletionPreflight?.can_delete || deletionConfirmation !== deletionPreflight.display_name)) return;
    deletionSubmitting = true;
    deletionError = null;
    try {
      const request = deletionRequest ?? {
        operation_id: crypto.randomUUID(), expected_workspace_updated_at: deletionPreflight!.expected_workspace_updated_at, confirmation: deletionConfirmation,
      };
      deletionRequest = request;
      sessionStorage.setItem(deletionStorageKey(), JSON.stringify(request));
      const operation = await startWorkspaceDeletion(workspaceId, request);
      if (disposed) return;
      deletionOperation = operation;
      await trackDeletion(operation.operation_id);
    } catch (error) {
      if (!disposed) deletionError = errorText(error, 'Workspace deletion failed.');
    } finally {
      if (!disposed) deletionSubmitting = false;
    }
  }
</script>

<div class="identity-settings">
  <h1 id="workspace-settings-title">Workspace Identity</h1>
  <section aria-labelledby="workspace-name-title" data-name-ready={!loading}>
    <div class="section-heading">
      <h2 id="workspace-name-title">Display name</h2>
      {#if workspaceMetadata && !editing}
        <button bind:this={editButton} type="button" disabled={deletionOpen || deletionSubmitting} onclick={editName}>Edit name</button>
      {/if}
    </div>
    {#if loading}<p class="muted" role="status">Loading workspace settings…</p>{/if}
    {#if metadataError}
      <p class="error" role="alert">{metadataError}</p>
      <button type="button" onclick={retryMetadata} disabled={loading}>Retry name</button>
    {/if}
    {#if workspaceMetadata}
      {#if editing}
        <form onsubmit={(event) => { event.preventDefault(); void submitWorkspaceName(); }} aria-label="Edit Workspace name">
          <label>New display name<input use:focusInput bind:value={displayNameDraft} required autocomplete="off" disabled={submitting || loading} aria-invalid={Boolean(nameError)} /></label>
          <p class="muted">Current: {workspaceMetadata.display_name}</p>
          {#if nameError}<p class="error" role="alert">{nameError}</p>{/if}
          <div class="actions">
            <button class="primary" type="submit" disabled={submitting || loading || !displayNameDraft.trim() || displayNameDraft === workspaceMetadata.display_name}>{submitting ? 'Saving…' : 'Save name'}</button>
            <button type="button" disabled={submitting || loading} onclick={cancelName}>Cancel</button>
            {#if nameError}<button type="button" disabled={submitting || loading} onclick={reloadName}>Reload saved name</button>{/if}
          </div>
        </form>
      {:else}
        <p class="saved-name">{workspaceMetadata.display_name}</p>
      {/if}
      {#if nameNotice}<p role="status">{nameNotice}</p>{/if}
      {#each diagnostics.filter((item) => item.severity !== 'info') as diagnostic}
        <p class:error={diagnostic.severity === 'error'} class:warning={diagnostic.severity === 'warning'} role="alert">{diagnostic.message}</p>
      {/each}
      <details class="technical">
        <summary>Technical details</summary>
        <dl>
          <div><dt>Source</dt><dd><code>{workspaceMetadata.source}</code></dd></div>
        </dl>
        {#each diagnostics.filter((item) => item.severity === 'info') as diagnostic}<p>{diagnostic.message}</p>{/each}
      </details>
    {/if}
  </section>

  {#if canDelete}
    <BevelLine decorative />
    <section aria-labelledby="workspace-identity-title" data-identity-ready={!identityLoading}>
      <div class="section-heading">
        <h2 id="workspace-identity-title">Public identity</h2>
        {#if signingIdentity?.public_bundle}<button bind:this={identityCopyButton} type="button" onclick={copyIdentityBundle}>Copy bundle</button>{/if}
      </div>
      {#if identityLoading}<p class="muted" role="status">Loading identity…</p>{/if}
      {#if identityError}<p class="error" role="alert">{identityError}</p>{/if}
      {#if !identityLoading && !signingIdentity}<button type="button" onclick={retryIdentity}>Retry identity</button>{/if}
      {#if signingIdentity?.identity.state === 'pending_provisioning'}
        <p class="warning">Not provisioned</p>
        <p class="muted">Provision a signing identity before connecting a Runtime.</p>
        <button bind:this={provisionButton} type="button" disabled={provisioningIdentity || deletionOpen || deletionSubmitting} onclick={provisionIdentity}>{provisioningIdentity ? 'Provisioning…' : 'Provision identity'}</button>
      {:else if signingIdentity?.public_bundle}
        <p>Active</p>
        {#if identityCopied}<p role="status">Public bundle copied.</p>{/if}
        {#if copyError}<p class="error" role="alert">{copyError}</p>{/if}
        <details class="technical">
          <summary>Public bundle and key details</summary>
          <p>Use this public bundle when connecting a Runtime. It contains no private key material.</p>
          <dl>
            <div><dt>Key</dt><dd><code>{signingIdentity.identity.key_id}</code></dd></div>
            <div><dt>Fingerprint</dt><dd><code>{signingIdentity.identity.public_key_fingerprint}</code></dd></div>
          </dl>
          <label>Public identity bundle<textarea readonly rows="9" value={identityBundleText} spellcheck="false"></textarea></label>
        </details>
      {/if}
    </section>
    <BevelLine decorative />
    <details class="deletion" bind:open={dangerOpen} ontoggle={(event) => { if (!event.currentTarget.open && !deletionSubmitting) deletionOpen = false; }}>
      <summary bind:this={dangerSummary}>{deletionSubmitting ? 'Deleting Workspace…' : 'Delete Workspace'}</summary>
      <p>Deleting this Workspace permanently removes its Workers, Workdirs, repositories, configuration, Memory, Tickets, and audit data. This cannot be undone.</p>
      {#if !deletionOpen}
        <button type="button" disabled={editing || submitting || provisioningIdentity} onclick={openDeletionConfirmation}>Review deletion impact</button>
      {:else}
        <section aria-labelledby="delete-workspace-title">
          <h2 id="delete-workspace-title">Delete {deletionPreflight?.display_name ?? deletionRequest?.confirmation ?? workspaceMetadata?.display_name ?? workspaceName}?</h2>
          {#if deletionLoading}<p role="status">Loading deletion impact…</p>{/if}
          {#if deletionPreflight}
            <ul>
              <li>{deletionPreflight.resources.workers} Workers</li><li>{deletionPreflight.resources.workdirs} Workdirs</li>
              <li>{deletionPreflight.resources.repositories} repositories</li><li>{deletionPreflight.resources.runtime_bindings} Runtime bindings</li>
              <li>{deletionPreflight.resources.secrets} secret records</li><li>{deletionPreflight.resources.artifacts} artifacts</li>
            </ul>
            {#each deletionPreflight.blockers as blocker}<p class="error" role="alert">{blocker.message}</p>{/each}
            {#if !deletionRequest}
              <label>Type {deletionPreflight.display_name} to confirm<input use:focusInput bind:value={deletionConfirmation} autocomplete="off" disabled={deletionSubmitting || !deletionPreflight.can_delete} /></label>
            {/if}
          {/if}
          {#if deletionOperation}
            <p role="status">Deletion state: {deletionOperation.state}</p>
            {#each deletionOperation.blockers as blocker}<p class="error" role="alert">{blocker.message}</p>{/each}
          {/if}
          {#if deletionError}<p class="error" role="alert">{deletionError}</p>{/if}
          <div class="actions">
            <button type="button" onclick={cancelDeletion} disabled={deletionSubmitting}>Cancel</button>
            {#if deletionPreflight || deletionRequest}
              <button class="danger" type="button" onclick={deleteWorkspace} disabled={deletionSubmitting || (!deletionRequest && (!deletionPreflight?.can_delete || deletionConfirmation !== deletionPreflight.display_name))}>{deletionSubmitting ? 'Deleting…' : deletionRequest ? 'Retry deletion' : 'Delete Workspace'}</button>
            {:else if !deletionLoading}
              <button type="button" onclick={openDeletionConfirmation}>Retry deletion check</button>
            {/if}
          </div>
        </section>
      {/if}
    </details>
  {/if}
</div>

<style>
  .identity-settings { display: grid; gap: var(--space-5); max-width: 52rem; min-width: 0; container-type: inline-size; }
  h1 { margin: 0; font-size: var(--font-size-title); line-height: var(--line-height-title); color: var(--text-strong); }
  h2 { margin: 0; font-size: var(--font-size-body); line-height: var(--line-height-body); color: var(--text-strong); }
  section, form { display: grid; gap: var(--space-3); min-width: 0; }
  .section-heading { display: flex; justify-content: space-between; align-items: center; gap: var(--space-3); }
  p { margin: 0; overflow-wrap: anywhere; }
  .muted, .technical { color: var(--text-muted); }
  .error { color: var(--danger); }
  .warning { color: var(--warning); }
  label { display: grid; gap: var(--space-1); min-width: 0; }
  input, textarea { width: 100%; min-width: 0; border: 1px solid var(--line); border-radius: var(--radius-soft); background: var(--bg-raised); color: var(--text-strong); padding: var(--space-2); font: inherit; }
  input[aria-invalid="true"] { border-color: var(--danger); }
  textarea { resize: vertical; font-family: var(--font-mono); font-size: var(--font-size-compact); line-height: var(--line-height-compact); }
  code { font-family: var(--font-mono); }
  .actions { display: flex; flex-wrap: wrap; gap: var(--space-2); }
  button { width: fit-content; max-width: 100%; border: 1px solid var(--line); border-radius: var(--radius-soft); padding: var(--space-2) var(--space-3); color: var(--text); background: transparent; cursor: pointer; font: inherit; }
  button.primary { background: var(--text-strong); color: var(--bg); border-color: var(--text-strong); }
  button.danger { color: var(--danger); border-color: var(--danger); }
  button:hover:not(:disabled), summary:hover { background: var(--interactive-hover); color: var(--text-strong); }
  button:active:not(:disabled), summary:active { background: var(--interactive-selected); color: var(--text-strong); }
  button:focus-visible, summary:focus-visible, input:focus-visible, textarea:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  button:disabled, input:disabled { opacity: 0.6; cursor: not-allowed; }
  summary { cursor: pointer; width: fit-content; max-width: 100%; color: var(--text); padding-block: var(--space-2); }
  details > :not(summary) { margin-top: var(--space-3); }
  dl { display: grid; gap: var(--space-2); margin: 0; font-size: var(--font-size-compact); line-height: var(--line-height-compact); }
  dl div { display: grid; grid-template-columns: 7rem minmax(0, 1fr); gap: var(--space-3); }
  dd { margin: 0; overflow-wrap: anywhere; }
  .deletion > summary { color: var(--danger); }
  ul { margin: 0; padding-left: var(--space-5); }
  @container (max-width: 28rem) { dl div { grid-template-columns: 1fr; gap: var(--space-1); } }
</style>
