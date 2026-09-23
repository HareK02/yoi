<script lang="ts">
  import { untrack } from 'svelte';
  import type {
    CreateRepositorySshCredentialRequest,
    DeleteRepositorySshCredentialRequest,
    DeleteRepositorySshHostTrustRequest,
    GenerateRepositorySshCredentialRequest,
    PutRepositorySshHostTrustRequest,
    RepositorySshCredential,
    RepositorySshHostTrust,
    RepositorySshPublicKey,
    RotateRepositorySshCredentialRequest,
  } from '$lib/generated/repository-access-api';
  import {
    REPOSITORY_ACCESS_MAX_RESPONSE_BYTES,
    parseRepositorySshCredential,
    parseRepositorySshHostTrust,
    parseRepositorySshPublicKey,
  } from '$lib/workspace/api/repository-access';
  import { readBoundedJson } from '$lib/workspace/api/http';
  import {
    repositoryAccessRequestError,
    validateCredentialRotation,
    validateGeneratedCredential,
    validateHostTrust,
    validateImportedCredential,
    type RepositoryAccessFieldErrors,
  } from '$lib/workspace/repository-access/model';
  import type { PageProps } from './$types';

  type Notice = { tone: 'success' | 'error'; text: string };
  type CredentialForm = 'generate' | 'import' | null;

  let { data }: PageProps = $props();
  let credentials = $state<RepositorySshCredential[]>(untrack(() => data.credentials));
  let publicKeys = $state<Record<string, RepositorySshPublicKey>>(
    Object.fromEntries(untrack(() => data.publicKeys).map((key) => [key.credential_id, key]))
  );
  let hostTrusts = $state<RepositorySshHostTrust[]>(untrack(() => data.hostTrusts));
  const accessProjection = untrack(() => data.accessProjection);
  const workspaceDefaultCredentialId = 'workspace-default';
  let activeOperation = $state<string | null>(null);
  let rotateCredentialId = $state<string | null>(null);
  const base = $derived(`/api/w/${encodeURIComponent(data.workspaceId)}/settings/repository-access`);
  const busy = $derived(activeOperation !== null);
  const selectedCredential = $derived(
    rotateCredentialId ? credentials.find((credential) => credential.credential_id === rotateCredentialId) ?? null : null
  );
  const sshRepository = $derived(data.repositories?.items.find((repository) => repository.source.kind === 'ssh') ?? null);
  const connectionTestHref = $derived(
    sshRepository
      ? `/w/${encodeURIComponent(data.workspaceId)}/repositories/${encodeURIComponent(sshRepository.repository_key)}`
      : `/w/${encodeURIComponent(data.workspaceId)}/settings/repositories`
  );

  let copiedCredentialId = $state<string | null>(null);
  let credentialNotice = $state<Notice | null>(null);
  let hostNotice = $state<Notice | null>(null);
  let publicKeyNotices = $state<Record<string, Notice>>({});

  let credentialForm = $state<CredentialForm>(null);
  let generateCredentialId = $state('');
  let generateCredentialName = $state('');
  let generateErrors = $state<RepositoryAccessFieldErrors>({});
  let credentialId = $state('');
  let credentialName = $state('');
  let privateKey = $state('');
  let passphrase = $state('');
  let importErrors = $state<RepositoryAccessFieldErrors>({});
  let rotatePrivateKey = $state('');
  let rotatePassphrase = $state('');
  let rotateErrors = $state<RepositoryAccessFieldErrors>({});

  let hostEditorOpen = $state(false);
  let hostTrustId = $state('');
  let hostname = $state('');
  let port = $state(22);
  let hostKey = $state('');
  let hostExpectedRevision = $state<number | null>(null);
  let hostErrors = $state<RepositoryAccessFieldErrors>({});

  function operationId(prefix: string): string {
    return `${prefix}-${crypto.randomUUID()}`;
  }

  function operationError(error: unknown, fallback: string): Notice {
    return { tone: 'error', text: error instanceof Error ? error.message : fallback };
  }

  function request<T>(
    path: string,
    method: string,
    body: unknown,
    parse: (value: unknown) => T,
    operation: string
  ): Promise<T>;
  function request(path: string, method: string, body: unknown, parse: null, operation: string): Promise<void>;
  async function request<T>(
    path: string,
    method: string,
    body: unknown,
    parse: ((value: unknown) => T) | null,
    operation: string
  ): Promise<T | undefined> {
    const response = await fetch(`${base}${path}`, {
      method,
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body)
    });
    if (!response.ok) {
      await response.body?.cancel();
      throw repositoryAccessRequestError(response.status, operation);
    }
    if (parse === null) {
      if (response.status !== 204) {
        await response.body?.cancel();
        throw new Error('Repository Access returned an unexpected response body.');
      }
      await response.body?.cancel();
      return undefined;
    }
    if (response.status === 204) throw new Error('Repository Access returned an unexpected empty response.');
    const payload = await readBoundedJson(response, REPOSITORY_ACCESS_MAX_RESPONSE_BYTES);
    return parse(payload);
  }

  async function loadPublicKey(credentialId: string): Promise<RepositorySshPublicKey> {
    const response = await fetch(
      `${base}/credentials/${encodeURIComponent(credentialId)}/public-key`,
      { headers: { accept: 'application/json' } }
    );
    if (!response.ok) {
      await response.body?.cancel();
      throw repositoryAccessRequestError(response.status, 'load this public key');
    }
    const body = await readBoundedJson(response, REPOSITORY_ACCESS_MAX_RESPONSE_BYTES);
    return parseRepositorySshPublicKey(body);
  }

  async function refreshPublicKey(credentialId: string): Promise<void> {
    try {
      const publicKey = await loadPublicKey(credentialId);
      publicKeys = { ...publicKeys, [credentialId]: publicKey };
    } catch (error) {
      publicKeyNotices = {
        ...publicKeyNotices,
        [credentialId]: operationError(error, 'The credential was saved, but its public key could not be loaded. Use Copy public key to retry.')
      };
    }
  }

  function openCredentialForm(form: Exclude<CredentialForm, null>) {
    credentialForm = form;
    rotateCredentialId = null;
    credentialNotice = null;
    generateErrors = {};
    importErrors = {};
  }

  function closeCredentialForms() {
    credentialForm = null;
    generateCredentialId = '';
    generateCredentialName = '';
    credentialId = '';
    credentialName = '';
    privateKey = '';
    passphrase = '';
    generateErrors = {};
    importErrors = {};
  }

  async function generateCredential() {
    generateErrors = validateGeneratedCredential({ credentialId: generateCredentialId, name: generateCredentialName });
    if (Object.keys(generateErrors).length > 0 || busy) return;
    activeOperation = 'generate-credential';
    credentialNotice = null;
    try {
      const body: GenerateRepositorySshCredentialRequest = {
        operation_id: operationId('credential-generate'),
        credential_id: generateCredentialId.trim(),
        name: generateCredentialName.trim()
      };
      const created = await request('/credentials/generate', 'POST', body, parseRepositorySshCredential, 'generate this SSH credential');
      credentials = [...credentials.filter((item) => item.credential_id !== created.credential_id), created]
        .sort((a, b) => a.credential_id.localeCompare(b.credential_id));
      await refreshPublicKey(created.credential_id);
      closeCredentialForms();
      credentialNotice = { tone: 'success', text: `Generated ${created.name}. Copy its public key before configuring the Repository provider.` };
    } catch (error) {
      credentialNotice = operationError(error, 'Failed to generate the SSH credential.');
    } finally {
      activeOperation = null;
    }
  }

  async function copyPublicKey(credential: RepositorySshCredential) {
    if (busy) return;
    activeOperation = `copy-${credential.credential_id}`;
    publicKeyNotices = { ...publicKeyNotices, [credential.credential_id]: { tone: 'success', text: 'Loading public key…' } };
    try {
      const publicKey = publicKeys[credential.credential_id] ?? await loadPublicKey(credential.credential_id);
      publicKeys = { ...publicKeys, [credential.credential_id]: publicKey };
      await navigator.clipboard.writeText(publicKey.public_key);
      copiedCredentialId = credential.credential_id;
      publicKeyNotices = { ...publicKeyNotices, [credential.credential_id]: { tone: 'success', text: 'Public key copied.' } };
      window.setTimeout(() => {
        if (copiedCredentialId === credential.credential_id) copiedCredentialId = null;
      }, 1500);
    } catch (error) {
      publicKeyNotices = { ...publicKeyNotices, [credential.credential_id]: operationError(error, 'Failed to copy the public key.') };
    } finally {
      activeOperation = null;
    }
  }

  async function createCredential() {
    importErrors = validateImportedCredential({ credentialId, name: credentialName, privateKey });
    if (Object.keys(importErrors).length > 0 || busy) return;
    activeOperation = 'import-credential';
    credentialNotice = null;
    try {
      const body: CreateRepositorySshCredentialRequest = {
        operation_id: operationId('credential-create'),
        credential_id: credentialId.trim(),
        name: credentialName.trim(),
        private_key: privateKey,
        passphrase: passphrase || null
      };
      const created = await request('/credentials', 'POST', body, parseRepositorySshCredential, 'import this SSH credential');
      credentials = [...credentials, created].sort((a, b) => a.credential_id.localeCompare(b.credential_id));
      await refreshPublicKey(created.credential_id);
      closeCredentialForms();
      credentialNotice = { tone: 'success', text: `Imported ${created.name}. The private key and passphrase fields were cleared.` };
    } catch (error) {
      credentialNotice = operationError(error, 'Credential import failed.');
    } finally {
      privateKey = '';
      passphrase = '';
      activeOperation = null;
    }
  }

  function openCredentialRotation(credential: RepositorySshCredential) {
    credentialForm = null;
    credentialNotice = null;
    rotateCredentialId = credential.credential_id;
    rotatePrivateKey = '';
    rotatePassphrase = '';
    rotateErrors = {};
  }

  function closeCredentialRotation() {
    rotateCredentialId = null;
    rotatePrivateKey = '';
    rotatePassphrase = '';
    rotateErrors = {};
  }

  async function rotateCredential(credential: RepositorySshCredential) {
    rotateErrors = validateCredentialRotation(rotatePrivateKey);
    if (Object.keys(rotateErrors).length > 0 || busy) return;
    activeOperation = `rotate-${credential.credential_id}`;
    credentialNotice = null;
    try {
      const body: RotateRepositorySshCredentialRequest = {
        operation_id: operationId('credential-rotate'),
        expected_revision: credential.current_revision,
        private_key: rotatePrivateKey,
        passphrase: rotatePassphrase || null
      };
      const rotated = await request(
        `/credentials/${encodeURIComponent(credential.credential_id)}/rotate`,
        'POST',
        body,
        parseRepositorySshCredential,
        `rotate ${credential.name}`
      );
      credentials = credentials.map((entry) => entry.credential_id === rotated.credential_id ? rotated : entry);
      await refreshPublicKey(rotated.credential_id);
      closeCredentialRotation();
      credentialNotice = { tone: 'success', text: `Rotated ${rotated.name}. Copy the new public key and update every external provider that uses it.` };
    } catch (error) {
      credentialNotice = operationError(error, 'Credential rotation failed.');
    } finally {
      rotatePrivateKey = '';
      rotatePassphrase = '';
      activeOperation = null;
    }
  }

  async function deleteCredential(credential: RepositorySshCredential) {
    if (credential.referenced_repositories.length > 0 || busy) return;
    if (!confirm(`Delete SSH credential ${credential.name} (${credential.credential_id})? This cannot be undone.`)) return;
    activeOperation = `delete-${credential.credential_id}`;
    credentialNotice = null;
    try {
      const body: DeleteRepositorySshCredentialRequest = {
        operation_id: operationId('credential-delete'),
        expected_revision: credential.current_revision
      };
      await request(`/credentials/${encodeURIComponent(credential.credential_id)}`, 'DELETE', body, null, `delete ${credential.name}`);
      credentials = credentials.filter((entry) => entry.credential_id !== credential.credential_id);
      const remainingPublicKeys = { ...publicKeys };
      delete remainingPublicKeys[credential.credential_id];
      publicKeys = remainingPublicKeys;
      credentialNotice = { tone: 'success', text: `Deleted ${credential.name}.` };
    } catch (error) {
      credentialNotice = operationError(error, 'Credential deletion failed.');
    } finally {
      activeOperation = null;
    }
  }

  function openNewHostTrust() {
    hostEditorOpen = true;
    hostTrustId = '';
    hostname = '';
    port = 22;
    hostKey = '';
    hostExpectedRevision = null;
    hostErrors = {};
    hostNotice = null;
  }

  function editHostTrust(hostTrust: RepositorySshHostTrust) {
    hostEditorOpen = true;
    hostTrustId = hostTrust.host_trust_id;
    hostname = hostTrust.hostname;
    port = hostTrust.port;
    hostKey = hostTrust.host_key;
    hostExpectedRevision = hostTrust.current_revision;
    hostErrors = {};
    hostNotice = null;
  }

  function closeHostEditor() {
    hostEditorOpen = false;
    hostTrustId = '';
    hostname = '';
    port = 22;
    hostKey = '';
    hostExpectedRevision = null;
    hostErrors = {};
  }

  async function saveHostTrust() {
    hostErrors = validateHostTrust({ hostTrustId, hostname, port, hostKey });
    if (Object.keys(hostErrors).length > 0 || busy) return;
    activeOperation = hostExpectedRevision === null ? 'add-host-trust' : `rotate-host-${hostTrustId}`;
    hostNotice = null;
    try {
      const body: PutRepositorySshHostTrustRequest = {
        operation_id: operationId('host-trust-save'),
        host_trust_id: hostTrustId.trim(),
        hostname: hostname.trim(),
        port,
        host_key: hostKey.trim(),
        expected_revision: hostExpectedRevision
      };
      const saved = await request('/host-trusts', 'POST', body, parseRepositorySshHostTrust, hostExpectedRevision === null ? 'pin this SSH host key' : 'rotate this pinned SSH host key');
      hostTrusts = hostExpectedRevision === null
        ? [...hostTrusts, saved].sort((a, b) => a.host_trust_id.localeCompare(b.host_trust_id))
        : hostTrusts.map((entry) => entry.host_trust_id === saved.host_trust_id ? saved : entry);
      const action = hostExpectedRevision === null ? 'Pinned' : 'Rotated';
      closeHostEditor();
      hostNotice = { tone: 'success', text: `${action} the key for ${saved.hostname}:${saved.port}. This records host identity; it does not prove Git authentication succeeded.` };
    } catch (error) {
      hostNotice = operationError(error, 'Failed to save the pinned host key.');
    } finally {
      activeOperation = null;
    }
  }

  async function deleteHostTrust(hostTrust: RepositorySshHostTrust) {
    if (hostTrust.referenced_repositories.length > 0 || busy) return;
    if (!confirm(`Delete the pinned host key for ${hostTrust.hostname}:${hostTrust.port}? Future SSH connections cannot verify this host until another key is pinned.`)) return;
    activeOperation = `delete-host-${hostTrust.host_trust_id}`;
    hostNotice = null;
    try {
      const body: DeleteRepositorySshHostTrustRequest = {
        operation_id: operationId('host-trust-delete'),
        expected_revision: hostTrust.current_revision
      };
      await request(`/host-trusts/${encodeURIComponent(hostTrust.host_trust_id)}`, 'DELETE', body, null, `delete the pin for ${hostTrust.hostname}:${hostTrust.port}`);
      hostTrusts = hostTrusts.filter((entry) => entry.host_trust_id !== hostTrust.host_trust_id);
      hostNotice = { tone: 'success', text: `Deleted the pin for ${hostTrust.hostname}:${hostTrust.port}.` };
    } catch (error) {
      hostNotice = operationError(error, 'Host key deletion failed.');
    } finally {
      activeOperation = null;
    }
  }
</script>

<svelte:head><title>Repository Access · Yoi Workspace</title></svelte:head>

<div class="repository-access-page" data-testid="repository-access-page">
  <section class="repository-access-section" aria-labelledby="repository-bindings-heading">
    <div class="repository-access-section-heading">
      <div>
        <h2 id="repository-bindings-heading">Repository bindings</h2>
        <p>Explicit overrides for Repository authentication and host identity.</p>
      </div>
    </div>
    {#if accessProjection.bindings.length === 0}
      <div class="repository-access-empty">
        <strong>No explicit bindings</strong>
        <p>SSH Repositories can still use the Workspace default credential and a unique pinned key that matches their host and port.</p>
      </div>
    {:else}
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <div class="repository-access-table-wrap" tabindex="0" role="region" aria-label="Explicit Repository access bindings">
        <table class="repository-access-table repository-access-binding-table">
          <thead><tr><th>Repository</th><th>Access</th><th>Authentication</th><th>Host identity</th></tr></thead>
          <tbody>
            {#each accessProjection.bindings as binding (binding.repository_key)}
              <tr>
                <td><strong>{binding.repository_key}</strong><small>Explicit binding</small></td>
                <td>{binding.access === 'read_write' ? 'Read and write' : 'Read only'}</td>
                <td><code>{workspaceDefaultCredentialId}</code><small>plus <code>{binding.credential_id}</code></small></td>
                <td><code>{binding.host_trust_id}</code></td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
    <details class="repository-access-technical">
      <summary>Technical details</summary>
      <dl>
        <div><dt>Config revision</dt><dd>{accessProjection.config_revision}</dd></div>
        <div><dt>Projection digest</dt><dd><code>{accessProjection.projection_digest}</code></dd></div>
      </dl>
    </details>
  </section>

  <section class="repository-access-section" aria-labelledby="credentials-heading" aria-busy={busy}>
    <div class="repository-access-section-heading">
      <div>
        <h2 id="credentials-heading">SSH credentials</h2>
        <p>Keys this Workspace can offer when it authenticates to an SSH Repository.</p>
      </div>
      {#if !data.credentialsError}
        <div class="repository-access-actions" aria-label="Add SSH credential">
          <button type="button" class="secondary" disabled={busy} onclick={() => openCredentialForm('generate')}>Generate credential</button>
          <button type="button" class="secondary" disabled={busy} onclick={() => openCredentialForm('import')}>Import credential</button>
        </div>
      {/if}
    </div>

    {#if credentialNotice}
      <p class="repository-access-notice {credentialNotice.tone}" role={credentialNotice.tone === 'error' ? 'alert' : 'status'}>{credentialNotice.text}</p>
    {/if}

    {#if data.credentialsError}
      <div class="repository-access-empty error" role="alert">
        <strong>SSH credentials unavailable</strong>
        <p>{data.credentialsError}</p>
        <button type="button" class="secondary" onclick={() => window.location.reload()}>Retry credentials</button>
      </div>
    {:else if credentials.length === 0}
      <div class="repository-access-empty">
        <strong>No SSH credentials are available</strong>
        <p>Generate a credential to get a new public key, or import an existing Ed25519 private key.</p>
      </div>
    {:else}
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <div class="repository-access-table-wrap" tabindex="0" role="region" aria-label="SSH credentials">
        <table class="repository-access-table repository-access-credential-table">
          <thead><tr><th>Credential</th><th>Role</th><th>Fingerprint</th><th>Used by</th><th><span class="visually-hidden">Actions</span></th></tr></thead>
          <tbody>
            {#each credentials as credential (credential.credential_id)}
              <tr>
                <td><strong>{credential.name}</strong><code>{credential.credential_id}</code></td>
                <td>{credential.credential_id === workspaceDefaultCredentialId ? 'Workspace default' : 'Additional'}</td>
                <td><code>{credential.public_key_algorithm}</code><code>{credential.public_key_fingerprint}</code></td>
                <td>
                  {#if credential.credential_id === workspaceDefaultCredentialId}
                    All SSH Repositories
                  {:else if credential.referenced_repositories.length > 0}
                    {credential.referenced_repositories.join(', ')}
                  {:else}
                    Not referenced
                  {/if}
                </td>
                <td>
                  <div class="repository-access-row-actions">
                    <button type="button" class="secondary" disabled={busy} onclick={() => void copyPublicKey(credential)}>
                      {activeOperation === `copy-${credential.credential_id}` ? 'Loading…' : copiedCredentialId === credential.credential_id ? 'Copied' : 'Copy public key'}
                    </button>
                    {#if credential.credential_id !== workspaceDefaultCredentialId}
                      <button type="button" class="secondary" disabled={busy} onclick={() => openCredentialRotation(credential)}>Rotate</button>
                      {#if credential.referenced_repositories.length === 0}
                        <button type="button" class="danger" disabled={busy} onclick={() => void deleteCredential(credential)}>Delete</button>
                      {/if}
                    {/if}
                  </div>
                  {#if publicKeyNotices[credential.credential_id]}
                    <small class:field-error={publicKeyNotices[credential.credential_id].tone === 'error'} role={publicKeyNotices[credential.credential_id].tone === 'error' ? 'alert' : 'status'}>{publicKeyNotices[credential.credential_id].text}</small>
                  {:else if credential.credential_id === workspaceDefaultCredentialId && data.defaultPublicKeyError}
                    <small class="field-error" role="alert">{data.defaultPublicKeyError} Use Copy public key to retry.</small>
                  {:else if credential.referenced_repositories.length > 0 && credential.credential_id !== workspaceDefaultCredentialId}
                    <small>Remove Repository references before deletion.</small>
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}

    {#if credentialForm === 'generate'}
      <form class="repository-access-form" novalidate onsubmit={(event) => { event.preventDefault(); void generateCredential(); }}>
        <div class="repository-access-form-heading"><div><h3>Generate SSH credential</h3><p>Creates an Ed25519 key. Copy the public key to the Repository provider after saving.</p></div></div>
        <div class="repository-access-form-grid">
          <label><span>Credential id</span><input bind:value={generateCredentialId} aria-invalid={Boolean(generateErrors.credentialId)} aria-describedby={generateErrors.credentialId ? 'generate-id-error' : undefined} autocomplete="off" />{#if generateErrors.credentialId}<small id="generate-id-error" class="field-error">{generateErrors.credentialId}</small>{/if}</label>
          <label><span>Name</span><input bind:value={generateCredentialName} aria-invalid={Boolean(generateErrors.name)} aria-describedby={generateErrors.name ? 'generate-name-error' : undefined} autocomplete="off" />{#if generateErrors.name}<small id="generate-name-error" class="field-error">{generateErrors.name}</small>{/if}</label>
        </div>
        <div class="repository-access-form-actions"><button type="submit" class="primary" disabled={busy}>{activeOperation === 'generate-credential' ? 'Generating…' : 'Generate credential'}</button><button type="button" class="secondary" disabled={busy} onclick={closeCredentialForms}>Cancel</button></div>
      </form>
    {:else if credentialForm === 'import'}
      <form class="repository-access-form" novalidate onsubmit={(event) => { event.preventDefault(); void createCredential(); }}>
        <div class="repository-access-form-heading"><div><h3>Import SSH credential</h3><p>The private key and optional passphrase are write-only and cleared after every save attempt.</p></div></div>
        <div class="repository-access-form-grid">
          <label><span>Credential id</span><input bind:value={credentialId} aria-invalid={Boolean(importErrors.credentialId)} aria-describedby={importErrors.credentialId ? 'import-id-error' : undefined} autocomplete="off" />{#if importErrors.credentialId}<small id="import-id-error" class="field-error">{importErrors.credentialId}</small>{/if}</label>
          <label><span>Name</span><input bind:value={credentialName} aria-invalid={Boolean(importErrors.name)} aria-describedby={importErrors.name ? 'import-name-error' : undefined} autocomplete="off" />{#if importErrors.name}<small id="import-name-error" class="field-error">{importErrors.name}</small>{/if}</label>
          <label class="wide"><span>OpenSSH private key (Ed25519)</span><textarea bind:value={privateKey} rows="8" autocomplete="off" data-web-ux-redact aria-invalid={Boolean(importErrors.privateKey)} aria-describedby={importErrors.privateKey ? 'import-key-error' : 'import-key-help'}></textarea><small id="import-key-help">Secret value; it cannot be read back after import.</small>{#if importErrors.privateKey}<small id="import-key-error" class="field-error">{importErrors.privateKey}</small>{/if}</label>
          <label class="wide"><span>Passphrase (optional)</span><input type="password" bind:value={passphrase} autocomplete="new-password" data-web-ux-redact /><small>Enter only when the private key is encrypted.</small></label>
        </div>
        <div class="repository-access-form-actions"><button type="submit" class="primary" disabled={busy}>{activeOperation === 'import-credential' ? 'Importing…' : 'Import credential'}</button><button type="button" class="secondary" disabled={busy} onclick={closeCredentialForms}>Cancel</button></div>
      </form>
    {/if}

    {#if selectedCredential}
      <form class="repository-access-form" novalidate onsubmit={(event) => { event.preventDefault(); void rotateCredential(selectedCredential); }}>
        <div class="repository-access-form-heading">
          <div><h3>Rotate {selectedCredential.name}</h3><p>Used by {selectedCredential.referenced_repositories.join(', ') || 'no Repositories'}. Existing provider keys must be replaced with the new public key after rotation.</p></div>
          <code>{selectedCredential.public_key_fingerprint}</code>
        </div>
        <div class="repository-access-form-grid">
          <label class="wide"><span>New OpenSSH private key (Ed25519)</span><textarea bind:value={rotatePrivateKey} rows="8" autocomplete="off" data-web-ux-redact aria-invalid={Boolean(rotateErrors.privateKey)} aria-describedby={rotateErrors.privateKey ? 'rotate-key-error' : 'rotate-key-help'}></textarea><small id="rotate-key-help">Secret value; it cannot be read back after rotation.</small>{#if rotateErrors.privateKey}<small id="rotate-key-error" class="field-error">{rotateErrors.privateKey}</small>{/if}</label>
          <label class="wide"><span>Passphrase (optional)</span><input type="password" bind:value={rotatePassphrase} autocomplete="new-password" data-web-ux-redact /></label>
        </div>
        <div class="repository-access-form-actions"><button type="submit" class="primary" disabled={busy}>{activeOperation === `rotate-${selectedCredential.credential_id}` ? 'Rotating…' : 'Rotate credential'}</button><button type="button" class="secondary" disabled={busy} onclick={closeCredentialRotation}>Cancel</button></div>
      </form>
    {/if}

    {#if credentials.length > 0 && !data.credentialsError}
      <details class="repository-access-technical">
        <summary>Credential revisions</summary>
        <dl>{#each credentials as credential (credential.credential_id)}<div><dt>{credential.credential_id}</dt><dd>Revision {credential.current_revision}</dd></div>{/each}</dl>
      </details>
    {/if}
  </section>

  <section class="repository-access-section" aria-labelledby="host-trust-heading" aria-busy={busy}>
    <div class="repository-access-section-heading">
      <div>
        <h2 id="host-trust-heading">Pinned SSH host keys</h2>
        <p>Keys used to verify the identity of a specific SSH host and port.</p>
      </div>
      {#if !data.hostTrustsError}<button type="button" class="secondary" disabled={busy} onclick={openNewHostTrust}>Add pinned key</button>{/if}
    </div>

    {#if hostNotice}<p class="repository-access-notice {hostNotice.tone}" role={hostNotice.tone === 'error' ? 'alert' : 'status'}>{hostNotice.text}</p>{/if}

    {#if data.hostTrustsError}
      <div class="repository-access-empty error" role="alert"><strong>Pinned host keys unavailable</strong><p>{data.hostTrustsError}</p><button type="button" class="secondary" onclick={() => window.location.reload()}>Retry pinned keys</button></div>
    {:else if hostTrusts.length === 0}
      <div class="repository-access-empty">
        <strong>No SSH host keys are pinned</strong>
        <p>Observe a host key from the Runtime that will clone the Repository, then compare its fingerprint before trusting it.</p>
        <a class="inline-link" href={connectionTestHref}>{sshRepository ? `Open ${sshRepository.repository_key} SSH connection test` : 'Choose an SSH Repository'}</a>
      </div>
    {:else}
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <div class="repository-access-table-wrap" tabindex="0" role="region" aria-label="Pinned SSH host keys">
        <table class="repository-access-table repository-access-host-table">
          <thead><tr><th>Host and port</th><th>Fingerprint</th><th>Used by</th><th><span class="visually-hidden">Actions</span></th></tr></thead>
          <tbody>
            {#each hostTrusts as hostTrust (hostTrust.host_trust_id)}
              <tr>
                <td><strong>{hostTrust.hostname}:{hostTrust.port}</strong><code>{hostTrust.host_trust_id}</code></td>
                <td><code>{hostTrust.key_algorithm}</code><code>{hostTrust.fingerprint}</code></td>
                <td>{hostTrust.referenced_repositories.join(', ') || 'Not referenced'}</td>
                <td>
                  <div class="repository-access-row-actions">
                    <button type="button" class="secondary" disabled={busy} onclick={() => editHostTrust(hostTrust)}>Rotate key</button>
                    {#if hostTrust.referenced_repositories.length === 0}<button type="button" class="danger" disabled={busy} onclick={() => void deleteHostTrust(hostTrust)}>Delete</button>{/if}
                  </div>
                  {#if hostTrust.referenced_repositories.length > 0}<small>Remove Repository references before deletion.</small>{/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}

    <p class="repository-access-guidance">A successful probe only observes a key. Saving a pin verifies future host identity; Git authentication succeeds only when a later Repository connection completes.</p>
    <a class="inline-link" href={connectionTestHref}>{sshRepository ? `Check ${sshRepository.repository_key} SSH connection` : 'Open Repository settings'}</a>

    {#if hostEditorOpen}
      <form class="repository-access-form" novalidate onsubmit={(event) => { event.preventDefault(); void saveHostTrust(); }}>
        <div class="repository-access-form-heading">
          <div><h3>{hostExpectedRevision === null ? 'Add pinned host key' : `Rotate ${hostname}:${port}`}</h3><p>{hostExpectedRevision === null ? 'Confirm the host, port, and fingerprint against a trusted source before saving.' : 'The new key replaces the current trusted identity for this host and port.'}</p></div>
          {#if hostExpectedRevision !== null}<code>{hostTrusts.find((entry) => entry.host_trust_id === hostTrustId)?.fingerprint}</code>{/if}
        </div>
        <div class="repository-access-form-grid">
          <label><span>Host trust id</span><input bind:value={hostTrustId} disabled={hostExpectedRevision !== null} aria-invalid={Boolean(hostErrors.hostTrustId)} aria-describedby={hostErrors.hostTrustId ? 'host-id-error' : undefined} />{#if hostErrors.hostTrustId}<small id="host-id-error" class="field-error">{hostErrors.hostTrustId}</small>{/if}</label>
          <label><span>Hostname</span><input bind:value={hostname} aria-invalid={Boolean(hostErrors.hostname)} aria-describedby={hostErrors.hostname ? 'hostname-error' : undefined} />{#if hostErrors.hostname}<small id="hostname-error" class="field-error">{hostErrors.hostname}</small>{/if}</label>
          <label><span>Port</span><input type="number" bind:value={port} aria-invalid={Boolean(hostErrors.port)} aria-describedby={hostErrors.port ? 'host-port-error' : undefined} />{#if hostErrors.port}<small id="host-port-error" class="field-error">{hostErrors.port}</small>{/if}</label>
          <label class="wide"><span>OpenSSH public host key (Ed25519)</span><textarea bind:value={hostKey} rows="4" aria-invalid={Boolean(hostErrors.hostKey)} aria-describedby={hostErrors.hostKey ? 'host-key-error' : undefined}></textarea>{#if hostErrors.hostKey}<small id="host-key-error" class="field-error">{hostErrors.hostKey}</small>{/if}</label>
        </div>
        <div class="repository-access-form-actions"><button type="submit" class="primary" disabled={busy}>{activeOperation === 'add-host-trust' || activeOperation?.startsWith('rotate-host-') ? 'Saving…' : hostExpectedRevision === null ? 'Pin host key' : 'Save new key'}</button><button type="button" class="secondary" disabled={busy} onclick={closeHostEditor}>Cancel</button></div>
      </form>
    {/if}

    {#if hostTrusts.length > 0 && !data.hostTrustsError}
      <details class="repository-access-technical">
        <summary>Host key revisions</summary>
        <dl>{#each hostTrusts as hostTrust (hostTrust.host_trust_id)}<div><dt>{hostTrust.host_trust_id}</dt><dd>Revision {hostTrust.current_revision}</dd></div>{/each}</dl>
      </details>
    {/if}
  </section>
</div>
