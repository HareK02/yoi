<script lang="ts">
  import { goto, invalidateAll } from '$app/navigation';
  import type {
    RevokeRuntimeTrustKeyRequest,
    RuntimeTrustKeyStatus,
  } from '$lib/generated/workspace-api';
  import {
    createRemoteRuntime,
    deleteRemoteRuntime,
    previewRuntimePublicKeyFingerprint,
    revealRuntimeTrustKey,
    revokeRuntimeTrustKey,
    RuntimeTrustConflictError,
    RuntimeTrustRouteFence,
    RuntimeTrustRequestError,
    type RuntimeTrustRouteOperation,
  } from '$lib/workspace/api/runtime-management';
  import type { PageProps } from './$types';

  type TrustAction = 'create' | 'replace' | 'reactivate';

  let { data }: PageProps = $props();
  let showPublicKey = $state(false);
  let revealedPublicKey = $state<string | null>(null);
  let publicKey = $state('');
  let deleteRuntimeConfirmation = $state('');
  let busyAction = $state<'save' | 'revoke' | 'reveal' | 'copy' | 'delete' | null>(null);
  let fieldError = $state<string | null>(null);
  let deleteRuntimeError = $state<string | null>(null);
  let requestError = $state<string | null>(null);
  let successMessage = $state<string | null>(null);
  let replacementFingerprint = $state<string | null>(null);
  let replacementFingerprintError = $state<string | null>(null);
  let fingerprintGeneration = 0;
  const routeFence = new RuntimeTrustRouteFence();
  let routeGeneration = 0;

  $effect(() => {
    const nextGeneration = routeFence.enter(data.runtimeId);
    if (nextGeneration === routeGeneration) return;
    routeGeneration = nextGeneration;
    fingerprintGeneration += 1;
    showPublicKey = false;
    revealedPublicKey = null;
    publicKey = '';
    deleteRuntimeConfirmation = '';
    busyAction = null;
    fieldError = null;
    deleteRuntimeError = null;
    requestError = null;
    successMessage = null;
    replacementFingerprint = null;
    replacementFingerprintError = null;
  });

  $effect(() => {
    const key = publicKey.trim();
    const generation = ++fingerprintGeneration;
    replacementFingerprint = null;
    replacementFingerprintError = null;
    if (!key) return;
    void previewRuntimePublicKeyFingerprint(key).then(
      (fingerprint) => {
        if (generation === fingerprintGeneration) replacementFingerprint = fingerprint;
      },
      (error) => {
        if (generation === fingerprintGeneration) {
          replacementFingerprintError = error instanceof Error
            ? error.message
            : String(error);
        }
      },
    );
  });

  function trustAction(status: RuntimeTrustKeyStatus): TrustAction {
    if (status === 'unconfigured') return 'create';
    if (status === 'revoked') return 'reactivate';
    return 'replace';
  }

  function actionLabel(action: TrustAction): string {
    switch (action) {
      case 'create': return 'Create Workspace trust';
      case 'replace': return 'Replace trusted key';
      case 'reactivate': return 'Reactivate with this key';
    }
  }

  function formatTimestamp(value: string | null | undefined): string {
    if (!value) return '—';
    const parsed = new Date(value);
    return Number.isNaN(parsed.getTime()) ? value : parsed.toLocaleString();
  }

  function utf8Bytes(value: string): number {
    return new TextEncoder().encode(value).byteLength;
  }

  async function reloadAuthority(): Promise<void> {
    await invalidateAll();
  }

  function isCurrentRoute(operation: RuntimeTrustRouteOperation): boolean {
    return routeFence.isCurrent(operation, data.runtimeId);
  }

  async function saveTrustKey(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (busyAction !== null || !data.runtimeDetail) return;

    fieldError = null;
    requestError = null;
    successMessage = null;

    const key = publicKey.trim();
    if (!key) {
      fieldError = 'Enter the Runtime public key.';
      return;
    }
    if (utf8Bytes(key) > 16 * 1024) {
      fieldError = 'Public key must be at most 16 KiB of UTF-8 text.';
      return;
    }
    if (replacementFingerprintError) {
      fieldError = replacementFingerprintError;
      return;
    }
    if (!replacementFingerprint) {
      fieldError = 'Wait for the replacement fingerprint preview before saving.';
      return;
    }

    const trust = data.runtimeDetail.trust_key;
    const action = trustAction(trust.status);

    const operation = routeFence.capture(data.runtimeId);
    busyAction = 'save';
    try {
      const binding = data.runtimeDetail.runtime.management.binding;
      if (!binding || !data.runtimeDetail.endpoint) {
        throw new RuntimeTrustRequestError(
          'The Workspace identity binding and authoritative Runtime endpoint are required.',
        );
      }
      await createRemoteRuntime(data.workspaceId, {
        public_bundle: {
          identity_id: operation.runtimeId,
          public_key: key,
        },
        display_name: data.runtimeDetail.runtime.label,
        endpoint: data.runtimeDetail.endpoint,
        expected_revision: binding.revision,
      });
      if (!isCurrentRoute(operation)) return;
      publicKey = '';
      showPublicKey = false;
      revealedPublicKey = null;
      successMessage = action === 'create'
        ? 'Workspace trust was created.'
        : action === 'replace'
        ? 'The trusted Runtime key was replaced.'
        : 'Workspace trust was reactivated.';
      await reloadAuthority();
    } catch (error) {
      if (!isCurrentRoute(operation)) return;
      if (error instanceof RuntimeTrustConflictError) {
        requestError = `${error.message} Authoritative Runtime trust has been reloaded.`;
        await reloadAuthority();
      } else if (error instanceof RuntimeTrustRequestError && error.field === 'public_key') {
        fieldError = error.message;
      } else {
        requestError = error instanceof Error ? error.message : 'Runtime trust update failed.';
      }
    } finally {
      if (isCurrentRoute(operation)) busyAction = null;
    }
  }

  async function revokeTrust(): Promise<void> {
    if (busyAction !== null || !data.runtimeDetail) return;
    const trust = data.runtimeDetail.trust_key;
    if (trust.revision == null || trust.status !== 'active') {
      requestError = 'Only active Workspace trust can be revoked.';
      return;
    }
    if (!trust.fingerprint) {
      requestError = 'The authoritative fingerprint is unavailable. Reload before revoking trust.';
      return;
    }

    fieldError = null;
    requestError = null;
    successMessage = null;
    const operation = routeFence.capture(data.runtimeId);
    busyAction = 'revoke';
    const request: RevokeRuntimeTrustKeyRequest = {
      expected_revision: trust.revision,
    };

    try {
      await revokeRuntimeTrustKey(
        data.workspaceId,
        operation.runtimeId,
        request,
        trust.fingerprint,
        trust.fingerprint,
      );
      if (!isCurrentRoute(operation)) return;
      publicKey = '';
      showPublicKey = false;
      revealedPublicKey = null;
      successMessage = 'Workspace trust was revoked.';
      await reloadAuthority();
    } catch (error) {
      if (!isCurrentRoute(operation)) return;
      if (error instanceof RuntimeTrustConflictError) {
        requestError = `${error.message} Authoritative Runtime trust has been reloaded.`;
        await reloadAuthority();
      } else {
        requestError = error instanceof Error ? error.message : 'Runtime trust revoke failed.';
      }
    } finally {
      if (isCurrentRoute(operation)) busyAction = null;
    }
  }

  async function deleteRegistration(): Promise<void> {
    if (busyAction !== null || !data.runtimeDetail) return;
    const runtime = data.runtimeDetail.runtime;
    if (runtime.management.built_in) return;
    if (deleteRuntimeConfirmation.trim() !== data.runtimeId) {
      deleteRuntimeError = 'Enter the Runtime ID exactly to confirm deletion.';
      return;
    }

    const operation = routeFence.capture(data.runtimeId);
    busyAction = 'delete';
    deleteRuntimeError = null;
    try {
      if (data.runtimeDetail.trust_key.status !== 'revoked') {
        const trust = data.runtimeDetail.trust_key;
        if (trust.revision == null || !trust.fingerprint) {
          throw new Error('Runtime trust revision and fingerprint are required before deletion.');
        }
        await revokeRuntimeTrustKey(
          data.workspaceId,
          operation.runtimeId,
          { expected_revision: trust.revision },
          trust.fingerprint,
          trust.fingerprint,
        );
        if (!isCurrentRoute(operation)) return;
      }
      await deleteRemoteRuntime(data.workspaceId, operation.runtimeId);
      if (!isCurrentRoute(operation)) return;
      await goto(`/w/${encodeURIComponent(data.workspaceId)}/settings/runtimes`, {
        replaceState: true,
      });
    } catch (error) {
      if (!isCurrentRoute(operation)) return;
      deleteRuntimeError = error instanceof Error
        ? error.message
        : 'Runtime registration deletion failed.';
    } finally {
      if (isCurrentRoute(operation)) busyAction = null;
    }
  }

  async function togglePublicKeyReveal(): Promise<void> {
    if (showPublicKey) {
      showPublicKey = false;
      revealedPublicKey = null;
      return;
    }
    if (busyAction !== null) return;
    const operation = routeFence.capture(data.runtimeId);
    busyAction = 'reveal';
    requestError = null;
    successMessage = null;
    try {
      const response = await revealRuntimeTrustKey(data.workspaceId, operation.runtimeId);
      if (!isCurrentRoute(operation)) return;
      revealedPublicKey = response.public_key;
      showPublicKey = true;
    } catch (error) {
      if (!isCurrentRoute(operation)) return;
      requestError = error instanceof Error ? error.message : 'Public key reveal failed.';
    } finally {
      if (isCurrentRoute(operation)) busyAction = null;
    }
  }

  async function copyPublicKey(): Promise<void> {
    if (busyAction !== null) return;
    const operation = routeFence.capture(data.runtimeId);
    busyAction = 'copy';
    requestError = null;
    successMessage = null;
    try {
      const response = await revealRuntimeTrustKey(data.workspaceId, operation.runtimeId);
      if (!isCurrentRoute(operation)) return;
      await navigator.clipboard.writeText(response.public_key);
      if (!isCurrentRoute(operation)) return;
      successMessage = 'Public key copied.';
    } catch (error) {
      if (!isCurrentRoute(operation)) return;
      requestError = error instanceof Error
        ? error.message
        : 'The browser could not copy the public key.';
    } finally {
      if (isCurrentRoute(operation)) busyAction = null;
    }
  }
</script>

<svelte:head>
  <title>{data.runtimeDetail?.runtime.label ?? data.runtimeId} · Runtime Settings · Yoi Workspace</title>
  <meta name="description" content="Runtime identity and Workspace trust settings" />
</svelte:head>

<section class="runtime-detail-page" aria-labelledby="runtime-detail-heading">
  <header class="page-header-row">
    <div>
      <a class="inline-link" href={`/w/${encodeURIComponent(data.workspaceId)}/settings/runtimes`}>Runtimes</a>
      <h1 id="runtime-detail-heading">{data.runtimeDetail?.runtime.label ?? data.runtimeId}</h1>
      <p><code>{data.runtimeId}</code></p>
    </div>
    <a class="button-link" href={`/w/${encodeURIComponent(data.workspaceId)}/settings/runtimes/${encodeURIComponent(data.runtimeId)}/workdirs`}>
      Workdirs
    </a>
  </header>

  {#if data.runtimeDetailError}
    <p class="section-state error">{data.runtimeDetailError}</p>
  {:else if !data.runtimeDetail}
    <p class="section-state">Loading Runtime…</p>
  {:else}
    {@const detail = data.runtimeDetail}
    {@const runtime = detail.runtime}
    {@const trust = detail.trust_key}
    {@const currentAction = trustAction(trust.status)}

    <section class="runtime-detail-section" aria-labelledby="runtime-identity-heading">
      <h2 id="runtime-identity-heading">Identity and binding</h2>
      <dl class="runtime-detail-grid">
        <div><dt>Runtime ID</dt><dd><code>{runtime.runtime_id}</code></dd></div>
        <div><dt>Kind</dt><dd>{runtime.kind}</dd></div>
        <div><dt>Endpoint</dt><dd>{detail.endpoint ?? 'Not configured'}</dd></div>
        <div><dt>Status</dt><dd>{runtime.status}</dd></div>
        <div><dt>Connection state</dt><dd>{runtime.management.binding?.connection_state ?? 'Not configured'}</dd></div>
        <div><dt>Workspace signing key</dt><dd><code>{runtime.management.binding?.workspace_key_id ?? '—'}</code></dd></div>
        <div><dt>Verified</dt><dd>{formatTimestamp(runtime.management.binding?.verification?.verified_at)}</dd></div>
        <div><dt>Verified binding revision</dt><dd>{runtime.management.binding?.verification?.binding_revision?.toString() ?? '—'}</dd></div>
        <div><dt>Last verification check</dt><dd>{runtime.management.binding?.verification?.last_outcome ?? '—'} · {formatTimestamp(runtime.management.binding?.verification?.last_checked_at)}</dd></div>
        <div><dt>Runtime key status</dt><dd>{trust.status}</dd></div>
        <div><dt>Fingerprint</dt><dd><code>{trust.fingerprint ?? '—'}</code></dd></div>
        <div><dt>Revision</dt><dd>{trust.revision?.toString() ?? '—'}</dd></div>
        <div><dt>Created</dt><dd>{formatTimestamp(trust.created_at)}</dd></div>
        <div><dt>Updated</dt><dd>{formatTimestamp(trust.updated_at)}</dd></div>
        <div><dt>Revoked</dt><dd>{formatTimestamp(trust.revoked_at)}</dd></div>
      </dl>
      {#if runtime.diagnostics.length > 0}
        <ul class="settings-diagnostics-list">
          {#each runtime.diagnostics as diagnostic}
            <li class:error={diagnostic.severity === 'error'} class:warning={diagnostic.severity === 'warning'}>
              <strong>{diagnostic.code}</strong>
              <span>{diagnostic.message}</span>
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    {#if data.workspace.permissions.manage_runtimes && !runtime.management.built_in}
      <section class="runtime-detail-section" aria-labelledby="runtime-trust-heading">
        <h2 id="runtime-trust-heading">Workspace trust</h2>

        {#if trust.status !== 'unconfigured'}
          <div class="runtime-public-key-actions">
            <button
              type="button"
              class="secondary"
              disabled={busyAction !== null}
              onclick={togglePublicKeyReveal}
            >
              {busyAction === 'reveal' ? 'Loading…' : showPublicKey ? 'Hide public key' : 'Reveal public key'}
            </button>
            <button type="button" class="secondary" disabled={busyAction !== null} onclick={copyPublicKey}>
              {busyAction === 'copy' ? 'Copying…' : 'Copy public key'}
            </button>
          </div>
          {#if showPublicKey && revealedPublicKey}
            <pre class="runtime-public-key"><code>{revealedPublicKey}</code></pre>
          {/if}
        {/if}

        <form class="runtime-trust-form" onsubmit={saveTrustKey}>
          <label for="runtime-public-key-input">Runtime public key</label>
          <textarea
            id="runtime-public-key-input"
            bind:value={publicKey}
            rows="5"
            autocomplete="off"
            spellcheck="false"
            aria-describedby={fieldError ? 'runtime-public-key-error' : undefined}
            aria-invalid={fieldError ? 'true' : undefined}
            placeholder="yoi-ed25519-pub:v1:…"
          ></textarea>

          <dl class="runtime-trust-comparison">
            <div>
              <dt>Current fingerprint</dt>
              <dd><code>{trust.fingerprint ?? 'Not configured'}</code></dd>
            </div>
            <div>
              <dt>Replacement fingerprint</dt>
              <dd><code>{replacementFingerprint ?? 'Enter a valid public key'}</code></dd>
            </div>
          </dl>
          {#if replacementFingerprintError}
            <p class="field-error">{replacementFingerprintError}</p>
          {/if}

          {#if fieldError}
            <p id="runtime-public-key-error" class="field-error">{fieldError}</p>
          {/if}
          <div class="settings-action-row">
            <button type="submit" disabled={busyAction !== null}>
              {busyAction === 'save' ? 'Saving…' : actionLabel(currentAction)}
            </button>
          </div>
        </form>

        <div class="runtime-revoke-row">
          <div>
            <strong>Revoke Workspace trust</strong>
            <p>Workspace trust only; this does not delete the Runtime process, Workers, or Workdirs.</p>
          </div>
          <button
            type="button"
            class="danger"
            disabled={busyAction !== null || trust.status !== 'active'}
            onclick={revokeTrust}
          >{busyAction === 'revoke' ? 'Revoking…' : 'Revoke trust'}</button>
        </div>

        {#if requestError}
          <p class="section-state error" role="alert">{requestError}</p>
        {/if}
        {#if successMessage}
          <p class="section-state success" role="status">{successMessage}</p>
        {/if}
      </section>
    {/if}

    <section class="runtime-detail-section" aria-labelledby="runtime-audit-heading">
      <h2 id="runtime-audit-heading">Recent trust audit</h2>
      {#if detail.recent_audit.length === 0}
        <p class="section-state">No trust changes are recorded.</p>
      {:else}
        <div class="runtime-audit-table-wrap">
          <table class="runtime-audit-table">
            <thead>
              <tr><th>Action</th><th>Revision</th><th>Fingerprint</th><th>Actor</th><th>Time</th></tr>
            </thead>
            <tbody>
              {#each detail.recent_audit as entry}
                <tr>
                  <td>{entry.action}</td>
                  <td>{entry.revision.toString()}</td>
                  <td><code>{entry.new_fingerprint ?? entry.old_fingerprint ?? '—'}</code></td>
                  <td><code>{entry.actor_account_id}</code></td>
                  <td>{formatTimestamp(entry.at)}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </section>

    {#if data.workspace.permissions.manage_runtimes && !runtime.management.built_in}
      <section class="runtime-detail-section runtime-danger-zone" aria-labelledby="runtime-delete-heading">
        <h2 id="runtime-delete-heading">Delete Runtime registration</h2>
        <p>
          Remove this Runtime binding from the current Workspace. This does not stop the Runtime process,
          delete its Workers or Workdirs, or revoke this Workspace on the Runtime host.
        </p>
        {#if trust.status !== 'revoked'}
          <p class="section-state warning">
            Deletion will revoke this Workspace trust first. Stop or move active Workers before continuing.
          </p>
        {/if}
        <label for="runtime-delete-confirmation">Confirm Runtime ID</label>
        <input
          id="runtime-delete-confirmation"
          bind:value={deleteRuntimeConfirmation}
          autocomplete="off"
          spellcheck="false"
          disabled={busyAction !== null}
          placeholder={data.runtimeId}
        />
        <small>Enter <code>{data.runtimeId}</code> exactly.</small>
        {#if deleteRuntimeError}
          <p class="section-state error" role="alert">{deleteRuntimeError}</p>
        {/if}
        <div class="settings-action-row">
          <button
            type="button"
            class="danger"
            disabled={busyAction !== null || deleteRuntimeConfirmation.trim() !== data.runtimeId}
            onclick={deleteRegistration}
          >{busyAction === 'delete'
              ? 'Deleting…'
              : trust.status === 'revoked'
              ? 'Delete registration'
              : 'Revoke trust and delete registration'}</button>
        </div>
      </section>
    {/if}
  {/if}
</section>
