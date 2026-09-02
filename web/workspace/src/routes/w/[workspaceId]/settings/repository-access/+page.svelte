<script lang="ts">
  import { untrack } from 'svelte';
  import type {
    CreateRepositorySshCredentialRequest,
    DeleteRepositorySshCredentialRequest,
    DeleteRepositorySshHostTrustRequest,
    PutRepositorySshHostTrustRequest,
    RepositorySshCredential,
    RepositorySshHostTrust,
    RotateRepositorySshCredentialRequest,
  } from '$lib/generated/repository-access-api';
  import {
    parseRepositorySshCredential,
    parseRepositorySshHostTrust,
  } from '$lib/workspace/api/repository-access';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
  let credentials = $state<RepositorySshCredential[]>(untrack(() => data.credentials));
  let hostTrusts = $state<RepositorySshHostTrust[]>(untrack(() => data.hostTrusts));
  const accessProjection = untrack(() => data.accessProjection);
  let message = $state<string | null>(null);
  let pending = $state(false);

  let credentialId = $state('');
  let credentialName = $state('');
  let privateKey = $state('');
  let passphrase = $state('');
  let rotateCredentialId = $state<string | null>(null);
  let rotatePrivateKey = $state('');
  let rotatePassphrase = $state('');

  let hostTrustId = $state('');
  let hostname = $state('');
  let port = $state(22);
  let hostKey = $state('');
  let hostExpectedRevision = $state<number | null>(null);

  const base = $derived(`/api/w/${encodeURIComponent(data.workspaceId)}/settings/repository-access`);

  function operationId(prefix: string): string {
    return `${prefix}-${crypto.randomUUID()}`;
  }

  function request<T>(
    path: string,
    method: string,
    body: unknown,
    parse: (value: unknown) => T
  ): Promise<T>;
  function request(path: string, method: string, body: unknown, parse: null): Promise<void>;
  async function request<T>(
    path: string,
    method: string,
    body: unknown,
    parse: ((value: unknown) => T) | null
  ): Promise<T | undefined> {
    const response = await fetch(`${base}${path}`, {
      method,
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body)
    });
    if (!response.ok) {
      throw new Error(`Repository Access request failed with status ${response.status}.`);
    }
    if (response.status === 204) return undefined;
    const payload: unknown = await response.json();
    if (parse === null) {
      throw new Error('Repository Access returned an unexpected response body.');
    }
    return parse(payload);
  }

  async function createCredential() {
    pending = true;
    message = null;
    try {
      const body: CreateRepositorySshCredentialRequest = {
        operation_id: operationId('credential-create'),
        credential_id: credentialId,
        name: credentialName,
        private_key: privateKey,
        passphrase: passphrase || null
      };
      const created = await request<RepositorySshCredential>(
        '/credentials',
        'POST',
        body,
        parseRepositorySshCredential
      );
      credentials = [...credentials, created].sort((a, b) => a.credential_id.localeCompare(b.credential_id));
      credentialId = '';
      credentialName = '';
      message = `Credential ${created.credential_id} created. Pasted secret fields were cleared.`;
    } catch (error) {
      message = error instanceof Error ? error.message : 'Credential creation failed';
    } finally {
      privateKey = '';
      passphrase = '';
      pending = false;
    }
  }

  async function rotateCredential(credential: RepositorySshCredential) {
    pending = true;
    message = null;
    try {
      const body: RotateRepositorySshCredentialRequest = {
        operation_id: operationId('credential-rotate'),
        expected_revision: credential.current_revision,
        private_key: rotatePrivateKey,
        passphrase: rotatePassphrase || null
      };
      const rotated = await request<RepositorySshCredential>(
        `/credentials/${encodeURIComponent(credential.credential_id)}/rotate`,
        'POST',
        body,
        parseRepositorySshCredential
      );
      credentials = credentials.map((entry) => entry.credential_id === rotated.credential_id ? rotated : entry);
      rotateCredentialId = null;
      message = `Credential ${rotated.credential_id} rotated to revision ${rotated.current_revision}. Pasted secret fields were cleared.`;
    } catch (error) {
      message = error instanceof Error ? error.message : 'Credential rotation failed';
    } finally {
      rotatePrivateKey = '';
      rotatePassphrase = '';
      pending = false;
    }
  }

  async function deleteCredential(credential: RepositorySshCredential) {
    if (!confirm(`Delete credential ${credential.credential_id}?`)) return;
    pending = true;
    message = null;
    try {
      const body: DeleteRepositorySshCredentialRequest = {
        operation_id: operationId('credential-delete'),
        expected_revision: credential.current_revision
      };
      await request(
        `/credentials/${encodeURIComponent(credential.credential_id)}`,
        'DELETE',
        body,
        null
      );
      credentials = credentials.filter((entry) => entry.credential_id !== credential.credential_id);
      message = `Credential ${credential.credential_id} deleted.`;
    } catch (error) {
      message = error instanceof Error ? error.message : 'Credential deletion failed';
    } finally {
      pending = false;
    }
  }

  async function createHostTrust() {
    pending = true;
    message = null;
    try {
      const body: PutRepositorySshHostTrustRequest = {
        operation_id: operationId('host-trust-create'),
        host_trust_id: hostTrustId,
        hostname,
        port,
        host_key: hostKey,
        expected_revision: hostExpectedRevision
      };
      const created = await request<RepositorySshHostTrust>(
        '/host-trusts',
        'POST',
        body,
        parseRepositorySshHostTrust
      );
      hostTrusts = hostExpectedRevision === null
        ? [...hostTrusts, created].sort((a, b) => a.host_trust_id.localeCompare(b.host_trust_id))
        : hostTrusts.map((entry) => entry.host_trust_id === created.host_trust_id ? created : entry);
      hostTrustId = '';
      hostname = '';
      port = 22;
      hostKey = '';
      hostExpectedRevision = null;
      message = `Host trust ${created.host_trust_id} saved at revision ${created.current_revision}.`;
    } catch (error) {
      message = error instanceof Error ? error.message : 'Host trust creation failed';
    } finally {
      pending = false;
    }
  }

  function editHostTrust(hostTrust: RepositorySshHostTrust) {
    hostTrustId = hostTrust.host_trust_id;
    hostname = hostTrust.hostname;
    port = hostTrust.port;
    hostKey = hostTrust.host_key;
    hostExpectedRevision = hostTrust.current_revision;
  }

  async function deleteHostTrust(hostTrust: RepositorySshHostTrust) {
    if (!confirm(`Delete host trust ${hostTrust.host_trust_id}?`)) return;
    pending = true;
    message = null;
    try {
      const body: DeleteRepositorySshHostTrustRequest = {
        operation_id: operationId('host-trust-delete'),
        expected_revision: hostTrust.current_revision
      };
      await request(
        `/host-trusts/${encodeURIComponent(hostTrust.host_trust_id)}`,
        'DELETE',
        body,
        null
      );
      hostTrusts = hostTrusts.filter((entry) => entry.host_trust_id !== hostTrust.host_trust_id);
      message = `Host trust ${hostTrust.host_trust_id} deleted.`;
    } catch (error) {
      message = error instanceof Error ? error.message : 'Host trust deletion failed';
    } finally {
      pending = false;
    }
  }
</script>

<svelte:head><title>Repository Access · Yoi Workspace</title></svelte:head>

<section class="card settings-section">
  <header class="settings-section-header">
    <div><p class="eyebrow">owner only</p><h2>Repository Access</h2></div>
    <span class="badge success">encrypted</span>
  </header>
  <p>Manage Workspace-scoped SSH credentials and pinned host keys. Private keys and passphrases are write-only and never returned by this page.</p>
  {#if message}<p class="status-message">{message}</p>{/if}

  <div class="settings-runtime-list">
    <h3>Active access projection</h3>
    <p>Config revision {accessProjection.config_revision} · <code>{accessProjection.projection_digest}</code></p>
    {#if accessProjection.bindings.length === 0}<p>No repository access bindings are active.</p>{/if}
    {#each accessProjection.bindings as binding (binding.repository_key)}
      <div class="card">
        <strong>{binding.repository_key}</strong>
        <p>{binding.access} · credential <code>{binding.credential_id}</code> · host trust <code>{binding.host_trust_id}</code></p>
      </div>
    {/each}
  </div>

  <div class="settings-runtime-list">
    <h3>SSH credentials</h3>
    {#if credentials.length === 0}<p>No credentials configured.</p>{/if}
    {#each credentials as credential (credential.credential_id)}
      <div class="card">
        <strong>{credential.name}</strong> <code>{credential.credential_id}</code>
        <p>{credential.public_key_algorithm} · {credential.public_key_fingerprint} · revision {credential.current_revision}</p>
        <p>References: {credential.referenced_repositories.join(', ') || 'none'}</p>
        <div class="settings-action-row">
          <button type="button" onclick={() => (rotateCredentialId = rotateCredentialId === credential.credential_id ? null : credential.credential_id)}>Rotate</button>
          <button type="button" class="danger" disabled={pending || credential.referenced_repositories.length > 0} onclick={() => void deleteCredential(credential)}>Delete</button>
        </div>
        {#if rotateCredentialId === credential.credential_id}
          <form class="settings-runtime-form" onsubmit={(event) => { event.preventDefault(); void rotateCredential(credential); }}>
            <label><span>New private key</span><textarea bind:value={rotatePrivateKey} required rows="8" autocomplete="off"></textarea></label>
            <label><span>Passphrase (only for an encrypted key)</span><input type="password" bind:value={rotatePassphrase} autocomplete="new-password" /></label>
            <button type="submit" disabled={pending}>Rotate credential</button>
          </form>
        {/if}
      </div>
    {/each}

    <form class="settings-runtime-form" onsubmit={(event) => { event.preventDefault(); void createCredential(); }}>
      <h3>Add SSH credential</h3>
      <label><span>Credential id</span><input bind:value={credentialId} required pattern="[A-Za-z0-9_.-]+" maxlength="128" /></label>
      <label><span>Name</span><input bind:value={credentialName} required maxlength="200" /></label>
      <label><span>OpenSSH private key (ssh-ed25519)</span><textarea bind:value={privateKey} required rows="10" autocomplete="off"></textarea></label>
      <label><span>Passphrase (only for an encrypted key)</span><input type="password" bind:value={passphrase} autocomplete="new-password" /></label>
      <button type="submit" disabled={pending}>Add credential</button>
    </form>
  </div>

  <div class="settings-runtime-list">
    <h3>Pinned SSH host keys</h3>
    {#if hostTrusts.length === 0}<p>No host trust records configured.</p>{/if}
    {#each hostTrusts as hostTrust (hostTrust.host_trust_id)}
      <div class="card">
        <strong>{hostTrust.hostname}:{hostTrust.port}</strong> <code>{hostTrust.host_trust_id}</code>
        <p>{hostTrust.key_algorithm} · {hostTrust.fingerprint} · revision {hostTrust.current_revision}</p>
        <p>References: {hostTrust.referenced_repositories.join(', ') || 'none'}</p>
        <div class="settings-action-row">
          <button type="button" onclick={() => editHostTrust(hostTrust)}>Rotate key</button>
          <button type="button" class="danger" disabled={pending || hostTrust.referenced_repositories.length > 0} onclick={() => void deleteHostTrust(hostTrust)}>Delete</button>
        </div>
      </div>
    {/each}

    <form class="settings-runtime-form" onsubmit={(event) => { event.preventDefault(); void createHostTrust(); }}>
      <h3>{hostExpectedRevision === null ? 'Add pinned host key' : 'Rotate pinned host key'}</h3>
      <label><span>Host trust id</span><input bind:value={hostTrustId} disabled={hostExpectedRevision !== null} required pattern="[A-Za-z0-9_.-]+" maxlength="128" /></label>
      <label><span>Hostname</span><input bind:value={hostname} required /></label>
      <label><span>Port</span><input type="number" bind:value={port} min="1" max="65535" required /></label>
      <label><span>OpenSSH public host key (ssh-ed25519)</span><textarea bind:value={hostKey} required rows="4"></textarea></label>
      <button type="submit" disabled={pending}>{hostExpectedRevision === null ? 'Add host key' : 'Save new revision'}</button>
      {#if hostExpectedRevision !== null}<button type="button" onclick={() => { hostTrustId = ''; hostname = ''; port = 22; hostKey = ''; hostExpectedRevision = null; }}>Cancel</button>{/if}
    </form>
  </div>
</section>
