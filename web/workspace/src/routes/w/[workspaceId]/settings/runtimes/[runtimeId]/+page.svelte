<script lang="ts">
  import { invalidateAll } from '$app/navigation';
  import type {
    PutRuntimeTrustKeyRequest,
    RevokeRuntimeTrustKeyRequest,
    RuntimeTrustKeyStatus,
  } from '$lib/generated/workspace-api';
  import {
    previewRuntimePublicKeyFingerprint,
    putRuntimeTrustKey,
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
  let fingerprintConfirmation = $state('');
  let revokeFingerprintConfirmation = $state('');
  let busyAction = $state<'save' | 'revoke' | 'reveal' | 'copy' | null>(null);
  let fieldError = $state<string | null>(null);
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
    fingerprintConfirmation = '';
    revokeFingerprintConfirmation = '';
    busyAction = null;
    fieldError = null;
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
    if (action !== 'create') {
      if (!trust.fingerprint) {
        requestError = 'The authoritative fingerprint is unavailable. Reload before changing trust.';
        return;
      }
      if (fingerprintConfirmation.trim() !== trust.fingerprint) {
        fieldError = 'Enter the current fingerprint exactly to confirm this change.';
        return;
      }
    }

    const request: PutRuntimeTrustKeyRequest = {
      public_key: key,
      expected_revision: trust.revision ?? null,
    };

    const operation = routeFence.capture(data.runtimeId);
    busyAction = 'save';
    try {
      await putRuntimeTrustKey(data.workspaceId, operation.runtimeId, request);
      if (!isCurrentRoute(operation)) return;
      publicKey = '';
      fingerprintConfirmation = '';
      revokeFingerprintConfirmation = '';
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
      fingerprintConfirmation = '';
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
    if (
      !trust.fingerprint ||
      revokeFingerprintConfirmation.trim() !== trust.fingerprint
    ) {
      fieldError = 'Enter the current fingerprint exactly before revoking Workspace trust.';
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
        revokeFingerprintConfirmation,
      );
      if (!isCurrentRoute(operation)) return;
      publicKey = '';
      fingerprintConfirmation = '';
      revokeFingerprintConfirmation = '';
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
        <div><dt>Binding status</dt><dd>{trust.status}</dd></div>
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

          {#if currentAction !== 'create'}
            <label for="runtime-fingerprint-confirmation">Confirm current fingerprint</label>
            <input
              id="runtime-fingerprint-confirmation"
              bind:value={fingerprintConfirmation}
              autocomplete="off"
              spellcheck="false"
              placeholder={trust.fingerprint ?? ''}
            />
            <small>Enter <code>{trust.fingerprint ?? 'the current fingerprint'}</code> exactly.</small>
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
            <label>
              Confirm current fingerprint
              <input
                bind:value={revokeFingerprintConfirmation}
                autocomplete="off"
                spellcheck="false"
                disabled={trust.status !== 'active' || busyAction !== null}
              />
              <small>Enter <code>{trust.fingerprint ?? 'the current fingerprint'}</code> exactly before revocation.</small>
            </label>
          </div>
          <button
            type="button"
            class="danger"
            disabled={
              busyAction !== null ||
              trust.status !== 'active' ||
              revokeFingerprintConfirmation.trim() !== trust.fingerprint
            }
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
  {/if}
</section>
