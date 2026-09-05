<script lang="ts">
  import { invalidateAll } from '$app/navigation';
  import type {
    PutRuntimeTrustKeyRequest,
    RevokeRuntimeTrustKeyRequest,
    RuntimeTrustKeyStatus,
  } from '$lib/generated/workspace-api';
  import {
    putRuntimeTrustKey,
    revokeRuntimeTrustKey,
    RuntimeTrustConflictError,
    RuntimeTrustRequestError,
  } from '$lib/workspace/api/runtime-management';
  import type { PageProps } from './$types';

  type TrustAction = 'create' | 'replace' | 'reactivate';

  let { data }: PageProps = $props();
  let revealPublicKey = $state(false);
  let publicKey = $state('');
  let fingerprintConfirmation = $state('');
  let busyAction = $state<'save' | 'revoke' | 'copy' | null>(null);
  let fieldError = $state<string | null>(null);
  let requestError = $state<string | null>(null);
  let successMessage = $state<string | null>(null);

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

    busyAction = 'save';
    try {
      await putRuntimeTrustKey(data.workspaceId, data.runtimeId, request);
      publicKey = '';
      fingerprintConfirmation = '';
      revealPublicKey = false;
      successMessage = action === 'create'
        ? 'Workspace trust was created.'
        : action === 'replace'
        ? 'The trusted Runtime key was replaced.'
        : 'Workspace trust was reactivated.';
      await reloadAuthority();
    } catch (error) {
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
      busyAction = null;
    }
  }

  async function revokeTrust(): Promise<void> {
    if (busyAction !== null || !data.runtimeDetail) return;
    const trust = data.runtimeDetail.trust_key;
    if (trust.revision == null || trust.status !== 'active') {
      requestError = 'Only active Workspace trust can be revoked.';
      return;
    }

    fieldError = null;
    requestError = null;
    successMessage = null;
    busyAction = 'revoke';
    const request: RevokeRuntimeTrustKeyRequest = {
      expected_revision: trust.revision,
    };

    try {
      await revokeRuntimeTrustKey(data.workspaceId, data.runtimeId, request);
      publicKey = '';
      fingerprintConfirmation = '';
      revealPublicKey = false;
      successMessage = 'Workspace trust was revoked.';
      await reloadAuthority();
    } catch (error) {
      if (error instanceof RuntimeTrustConflictError) {
        requestError = `${error.message} Authoritative Runtime trust has been reloaded.`;
        await reloadAuthority();
      } else {
        requestError = error instanceof Error ? error.message : 'Runtime trust revoke failed.';
      }
    } finally {
      busyAction = null;
    }
  }

  async function copyPublicKey(): Promise<void> {
    const key = data.runtimeDetail?.trust_key.public_key;
    if (!key || busyAction !== null) return;
    busyAction = 'copy';
    requestError = null;
    try {
      await navigator.clipboard.writeText(key);
      successMessage = 'Public key copied.';
    } catch {
      requestError = 'The browser could not copy the public key.';
    } finally {
      busyAction = null;
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

    {#if data.workspace.permissions.manage_runtimes}
      <section class="runtime-detail-section" aria-labelledby="runtime-trust-heading">
        <h2 id="runtime-trust-heading">Workspace trust</h2>

        {#if trust.public_key}
          <div class="runtime-public-key-actions">
            <button type="button" class="secondary" onclick={() => revealPublicKey = !revealPublicKey}>
              {revealPublicKey ? 'Hide public key' : 'Reveal public key'}
            </button>
            <button type="button" class="secondary" disabled={busyAction !== null} onclick={copyPublicKey}>
              {busyAction === 'copy' ? 'Copying…' : 'Copy public key'}
            </button>
          </div>
          {#if revealPublicKey}
            <pre class="runtime-public-key"><code>{trust.public_key}</code></pre>
          {/if}
        {:else if trust.status !== 'unconfigured'}
          <p class="section-state">The public key was not included in this authorized response.</p>
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
            placeholder="ssh-ed25519 …"
          ></textarea>

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
  {/if}
</section>
