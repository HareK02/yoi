<script lang="ts">
  import { tick, untrack } from 'svelte';
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
  } from '#lib/generated/repository-access-api.ts';
  import {
    REPOSITORY_ACCESS_MAX_RESPONSE_BYTES,
    parseRepositorySshCredential,
    parseRepositorySshHostTrust,
    parseRepositorySshPublicKey,
  } from '#lib/workspace/api/repository-access.ts';
  import { readBoundedJson } from '#lib/workspace/api/http.ts';
  import {
    repositoryAccessRequestError,
    validateCredentialRotation,
    validateGeneratedCredential,
    validateHostTrust,
    validateImportedCredential,
    type RepositoryAccessFieldErrors,
  } from '#lib/workspace/repository-access/model.ts';
  import type { PageProps } from './$types';

  type Notice = { tone: 'success' | 'error'; text: string };
  type CredentialForm = 'generate' | 'import' | null;

  let { data }: PageProps = $props();
  const workspaceDefaultCredentialId = 'workspace-default';

  function sortCredentials(items: RepositorySshCredential[]): RepositorySshCredential[] {
    return [...items].sort((a, b) => {
      if (a.credential_id === workspaceDefaultCredentialId) return -1;
      if (b.credential_id === workspaceDefaultCredentialId) return 1;
      return a.credential_id.localeCompare(b.credential_id);
    });
  }

  let credentials = $state<RepositorySshCredential[]>(sortCredentials(untrack(() => data.credentials)));
  let publicKeys = $state<Record<string, RepositorySshPublicKey>>(
    Object.fromEntries(untrack(() => data.publicKeys).map((key) => [key.credential_id, key]))
  );
  let hostTrusts = $state<RepositorySshHostTrust[]>(untrack(() => data.hostTrusts));
  let accessProjection = $state(untrack(() => data.accessProjection));
  let pageEpoch = 0;
  let pendingOperations = $state<string[]>([]);
  let rotateCredentialId = $state<string | null>(null);
  const base = $derived(`/api/w/${encodeURIComponent(data.workspaceId)}/settings/repository-access`);
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
  let credentialFormNotice = $state<Notice | null>(null);
  let credentialRowNotices = $state<Record<string, Notice>>({});
  let hostNotice = $state<Notice | null>(null);
  let hostFormNotice = $state<Notice | null>(null);
  let hostRowNotices = $state<Record<string, Notice>>({});
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
  let generateCredentialIdInput = $state<HTMLInputElement>();
  let importCredentialIdInput = $state<HTMLInputElement>();
  let rotatePrivateKeyInput = $state<HTMLTextAreaElement>();
  let credentialFormElement = $state<HTMLFormElement>();
  let rotationFormElement = $state<HTMLFormElement>();
  let credentialReturnFocus: HTMLElement | null = null;

  let hostEditorOpen = $state(false);
  let hostTrustId = $state('');
  let hostname = $state('');
  let port = $state(22);
  let hostKey = $state('');
  let hostExpectedRevision = $state<number | null>(null);
  let hostErrors = $state<RepositoryAccessFieldErrors>({});
  let hostTrustIdInput = $state<HTMLInputElement>();
  let hostKeyInput = $state<HTMLTextAreaElement>();
  let hostFormElement = $state<HTMLFormElement>();
  let hostReturnFocus: HTMLElement | null = null;
  const editingHostTrust = $derived(
    hostExpectedRevision === null ? null : hostTrusts.find((entry) => entry.host_trust_id === hostTrustId) ?? null
  );

  function resetPageState(next: PageProps['data']): void {
    pageEpoch += 1;
    credentials = sortCredentials(next.credentials);
    publicKeys = Object.fromEntries(next.publicKeys.map((key) => [key.credential_id, key]));
    hostTrusts = next.hostTrusts;
    accessProjection = next.accessProjection;
    pendingOperations = [];
    rotateCredentialId = null;
    copiedCredentialId = null;
    credentialNotice = null;
    credentialFormNotice = null;
    credentialRowNotices = {};
    hostNotice = null;
    hostFormNotice = null;
    hostRowNotices = {};
    publicKeyNotices = {};
    credentialForm = null;
    generateCredentialId = '';
    generateCredentialName = '';
    generateErrors = {};
    credentialId = '';
    credentialName = '';
    privateKey = '';
    passphrase = '';
    importErrors = {};
    rotatePrivateKey = '';
    rotatePassphrase = '';
    rotateErrors = {};
    credentialReturnFocus = null;
    hostEditorOpen = false;
    hostTrustId = '';
    hostname = '';
    port = 22;
    hostKey = '';
    hostExpectedRevision = null;
    hostErrors = {};
    hostReturnFocus = null;
  }

  $effect(() => {
    const next = data;
    untrack(() => resetPageState(next));
  });

  class StaleRepositoryAccessRequestError extends Error {}

  function operationId(prefix: string): string {
    return `${prefix}-${crypto.randomUUID()}`;
  }

  function operationError(error: unknown, fallback: string): Notice {
    return { tone: 'error', text: error instanceof Error ? error.message : fallback };
  }

  function pendingOperationKey(operation: string): string {
    return `${pageEpoch}:${operation}`;
  }

  function isPending(operation: string): boolean {
    return pendingOperations.includes(pendingOperationKey(operation));
  }

  function startOperation(operation: string): string | null {
    const key = pendingOperationKey(operation);
    if (pendingOperations.includes(key)) return null;
    pendingOperations = [...pendingOperations, key];
    return key;
  }

  function finishOperation(operationKey: string): void {
    pendingOperations = pendingOperations.filter((entry) => entry !== operationKey);
  }

  function isCurrentOperation(operationKey: string): boolean {
    return operationKey.startsWith(`${pageEpoch}:`);
  }

  function requireCurrentOperation(operationKey: string): void {
    if (!isCurrentOperation(operationKey)) throw new StaleRepositoryAccessRequestError();
  }

  function credentialLifecyclePending(credentialId: string): boolean {
    return isPending(`copy-${credentialId}`) || isPending(`rotate-${credentialId}`) || isPending(`delete-${credentialId}`);
  }

  function credentialEditorPending(): boolean {
    const credentialRotationPrefix = `${pageEpoch}:rotate-`;
    const hostRotationPrefix = `${pageEpoch}:rotate-host-`;
    return isPending('generate-credential') || isPending('import-credential') || pendingOperations.some((entry) => entry.startsWith(credentialRotationPrefix) && !entry.startsWith(hostRotationPrefix));
  }

  function hostEditorPending(): boolean {
    return isPending('add-host-trust') || pendingOperations.some((entry) => entry.startsWith(`${pageEpoch}:rotate-host-`));
  }

  function currentFocus(): HTMLElement | null {
    return document.activeElement instanceof HTMLElement ? document.activeElement : null;
  }

  function focusIsInside(element: HTMLElement | undefined): boolean {
    return element?.contains(document.activeElement) ?? false;
  }

  async function restoreFocus(target: HTMLElement | null, fallbackId?: string): Promise<void> {
    await tick();
    const fallback = fallbackId ? document.getElementById(fallbackId) : null;
    const destination = fallback ?? (target?.isConnected ? target : null);
    destination?.focus();
  }

  async function focusFirstInvalid(form: HTMLFormElement | undefined): Promise<void> {
    await tick();
    form?.querySelector<HTMLElement>('[aria-invalid="true"]')?.focus();
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
    const epoch = pageEpoch;
    const response = await fetch(`${base}${path}`, {
      method,
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body)
    });
    if (epoch !== pageEpoch) {
      await response.body?.cancel();
      throw new StaleRepositoryAccessRequestError();
    }
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
    if (epoch !== pageEpoch) throw new StaleRepositoryAccessRequestError();
    return parse(payload);
  }

  async function loadPublicKey(credentialId: string): Promise<RepositorySshPublicKey> {
    const epoch = pageEpoch;
    const response = await fetch(
      `${base}/credentials/${encodeURIComponent(credentialId)}/public-key`,
      { headers: { accept: 'application/json' } }
    );
    if (epoch !== pageEpoch) {
      await response.body?.cancel();
      throw new StaleRepositoryAccessRequestError();
    }
    if (!response.ok) {
      await response.body?.cancel();
      throw repositoryAccessRequestError(response.status, 'load this public key');
    }
    const body = await readBoundedJson(response, REPOSITORY_ACCESS_MAX_RESPONSE_BYTES);
    if (epoch !== pageEpoch) throw new StaleRepositoryAccessRequestError();
    return parseRepositorySshPublicKey(body);
  }

  async function refreshPublicKey(credentialId: string): Promise<void> {
    const epoch = pageEpoch;
    try {
      const publicKey = await loadPublicKey(credentialId);
      if (epoch !== pageEpoch) throw new StaleRepositoryAccessRequestError();
      publicKeys = { ...publicKeys, [credentialId]: publicKey };
    } catch (error) {
      if (error instanceof StaleRepositoryAccessRequestError || epoch !== pageEpoch) throw new StaleRepositoryAccessRequestError();
      const remainingPublicKeys = { ...publicKeys };
      delete remainingPublicKeys[credentialId];
      publicKeys = remainingPublicKeys;
      publicKeyNotices = {
        ...publicKeyNotices,
        [credentialId]: operationError(error, 'The credential was saved, but its public key could not be loaded. Use Copy public key to retry.')
      };
    }
  }

  async function openCredentialForm(form: Exclude<CredentialForm, null>) {
    credentialReturnFocus = currentFocus();
    credentialForm = form;
    rotateCredentialId = null;
    credentialNotice = null;
    credentialFormNotice = null;
    generateErrors = {};
    importErrors = {};
    await tick();
    (form === 'generate' ? generateCredentialIdInput : importCredentialIdInput)?.focus();
  }

  async function closeCredentialForms(restore = true) {
    const returnFocus = credentialReturnFocus;
    const shouldRestoreFocus = restore && focusIsInside(credentialFormElement);
    credentialForm = null;
    generateCredentialId = '';
    generateCredentialName = '';
    credentialId = '';
    credentialName = '';
    privateKey = '';
    passphrase = '';
    generateErrors = {};
    importErrors = {};
    credentialFormNotice = null;
    credentialReturnFocus = null;
    if (shouldRestoreFocus) await restoreFocus(returnFocus);
  }

  async function generateCredential() {
    generateErrors = validateGeneratedCredential({ credentialId: generateCredentialId, name: generateCredentialName });
    if (Object.keys(generateErrors).length > 0) {
      await focusFirstInvalid(credentialFormElement);
      return;
    }
    const operation = startOperation('generate-credential');
    if (!operation) return;
    credentialFormNotice = null;
    try {
      const body: GenerateRepositorySshCredentialRequest = {
        operation_id: operationId('credential-generate'),
        credential_id: generateCredentialId.trim(),
        name: generateCredentialName.trim()
      };
      const created = await request('/credentials/generate', 'POST', body, parseRepositorySshCredential, 'generate this SSH credential');
      requireCurrentOperation(operation);
      credentials = sortCredentials([...credentials.filter((item) => item.credential_id !== created.credential_id), created]);
      await refreshPublicKey(created.credential_id);
      requireCurrentOperation(operation);
      await closeCredentialForms();
      requireCurrentOperation(operation);
      credentialNotice = { tone: 'success', text: `Generated ${created.name}. Copy its public key before configuring the Repository provider.` };
    } catch (error) {
      if (error instanceof StaleRepositoryAccessRequestError) return;
      credentialFormNotice = operationError(error, 'Failed to generate the SSH credential.');
    } finally {
      finishOperation(operation);
    }
  }

  async function copyPublicKey(credential: RepositorySshCredential) {
    if (credentialLifecyclePending(credential.credential_id)) return;
    const operation = startOperation(`copy-${credential.credential_id}`);
    if (!operation) return;
    publicKeyNotices = { ...publicKeyNotices, [credential.credential_id]: { tone: 'success', text: 'Loading public key…' } };
    try {
      const publicKey = publicKeys[credential.credential_id] ?? await loadPublicKey(credential.credential_id);
      requireCurrentOperation(operation);
      publicKeys = { ...publicKeys, [credential.credential_id]: publicKey };
      await navigator.clipboard.writeText(publicKey.public_key);
      requireCurrentOperation(operation);
      copiedCredentialId = credential.credential_id;
      publicKeyNotices = { ...publicKeyNotices, [credential.credential_id]: { tone: 'success', text: 'Public key copied.' } };
      window.setTimeout(() => {
        if (isCurrentOperation(operation) && copiedCredentialId === credential.credential_id) copiedCredentialId = null;
      }, 1500);
    } catch (error) {
      if (error instanceof StaleRepositoryAccessRequestError) return;
      publicKeyNotices = { ...publicKeyNotices, [credential.credential_id]: operationError(error, 'Failed to copy the public key.') };
    } finally {
      finishOperation(operation);
    }
  }

  async function createCredential() {
    importErrors = validateImportedCredential({ credentialId, name: credentialName, privateKey });
    if (Object.keys(importErrors).length > 0) {
      await focusFirstInvalid(credentialFormElement);
      return;
    }
    const operation = startOperation('import-credential');
    if (!operation) return;
    credentialFormNotice = null;
    try {
      const body: CreateRepositorySshCredentialRequest = {
        operation_id: operationId('credential-create'),
        credential_id: credentialId.trim(),
        name: credentialName.trim(),
        private_key: privateKey,
        passphrase: passphrase || null
      };
      const created = await request('/credentials', 'POST', body, parseRepositorySshCredential, 'import this SSH credential');
      requireCurrentOperation(operation);
      credentials = sortCredentials([...credentials, created]);
      await refreshPublicKey(created.credential_id);
      requireCurrentOperation(operation);
      await closeCredentialForms();
      requireCurrentOperation(operation);
      credentialNotice = { tone: 'success', text: `Imported ${created.name}. The private key and passphrase fields were cleared.` };
    } catch (error) {
      if (error instanceof StaleRepositoryAccessRequestError) return;
      credentialFormNotice = operationError(error, 'Credential import failed.');
    } finally {
      if (isCurrentOperation(operation)) {
        privateKey = '';
        passphrase = '';
      }
      finishOperation(operation);
    }
  }

  async function openCredentialRotation(credential: RepositorySshCredential) {
    if (credentialLifecyclePending(credential.credential_id)) return;
    credentialReturnFocus = currentFocus();
    credentialForm = null;
    credentialFormNotice = null;
    rotateCredentialId = credential.credential_id;
    rotatePrivateKey = '';
    rotatePassphrase = '';
    rotateErrors = {};
    await tick();
    rotatePrivateKeyInput?.focus();
  }

  async function closeCredentialRotation(restore = true) {
    const returnFocus = credentialReturnFocus;
    const shouldRestoreFocus = restore && focusIsInside(rotationFormElement);
    const fallbackId = rotateCredentialId ? `rotate-credential-${rotateCredentialId}` : undefined;
    rotateCredentialId = null;
    rotatePrivateKey = '';
    rotatePassphrase = '';
    rotateErrors = {};
    credentialFormNotice = null;
    credentialReturnFocus = null;
    if (shouldRestoreFocus) await restoreFocus(returnFocus, fallbackId);
  }

  async function rotateCredential(credential: RepositorySshCredential) {
    if (credentialLifecyclePending(credential.credential_id)) return;
    rotateErrors = validateCredentialRotation(rotatePrivateKey);
    if (Object.keys(rotateErrors).length > 0) {
      await focusFirstInvalid(rotationFormElement);
      return;
    }
    const operation = startOperation(`rotate-${credential.credential_id}`);
    if (!operation) return;
    credentialFormNotice = null;
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
      requireCurrentOperation(operation);
      credentials = credentials.map((entry) => entry.credential_id === rotated.credential_id ? rotated : entry);
      await refreshPublicKey(rotated.credential_id);
      requireCurrentOperation(operation);
      finishOperation(operation);
      await closeCredentialRotation();
      requireCurrentOperation(operation);
      credentialRowNotices = {
        ...credentialRowNotices,
        [rotated.credential_id]: { tone: 'success', text: `Rotated ${rotated.name}. Copy the new public key and update every external provider that uses it.` }
      };
    } catch (error) {
      if (error instanceof StaleRepositoryAccessRequestError) return;
      credentialFormNotice = operationError(error, 'Credential rotation failed.');
    } finally {
      if (isCurrentOperation(operation)) {
        rotatePrivateKey = '';
        rotatePassphrase = '';
      }
      finishOperation(operation);
    }
  }

  async function deleteCredential(credential: RepositorySshCredential) {
    if (credential.referenced_repositories.length > 0 || credentialLifecyclePending(credential.credential_id)) return;
    const operationName = `delete-${credential.credential_id}`;
    if (!confirm(`Delete SSH credential ${credential.name} (${credential.credential_id})? This cannot be undone.`)) return;
    const operation = startOperation(operationName);
    if (!operation) return;
    credentialRowNotices = { ...credentialRowNotices, [credential.credential_id]: { tone: 'success', text: 'Deleting credential…' } };
    try {
      const body: DeleteRepositorySshCredentialRequest = {
        operation_id: operationId('credential-delete'),
        expected_revision: credential.current_revision
      };
      await request(`/credentials/${encodeURIComponent(credential.credential_id)}`, 'DELETE', body, null, `delete ${credential.name}`);
      requireCurrentOperation(operation);
      credentials = credentials.filter((entry) => entry.credential_id !== credential.credential_id);
      const remainingPublicKeys = { ...publicKeys };
      delete remainingPublicKeys[credential.credential_id];
      publicKeys = remainingPublicKeys;
      credentialNotice = { tone: 'success', text: `Deleted ${credential.name}.` };
    } catch (error) {
      if (error instanceof StaleRepositoryAccessRequestError) return;
      credentialRowNotices = { ...credentialRowNotices, [credential.credential_id]: operationError(error, 'Credential deletion failed.') };
    } finally {
      finishOperation(operation);
    }
  }

  async function openNewHostTrust() {
    hostReturnFocus = currentFocus();
    hostEditorOpen = true;
    hostTrustId = '';
    hostname = '';
    port = 22;
    hostKey = '';
    hostExpectedRevision = null;
    hostErrors = {};
    hostFormNotice = null;
    await tick();
    hostTrustIdInput?.focus();
  }

  async function editHostTrust(hostTrust: RepositorySshHostTrust) {
    hostReturnFocus = currentFocus();
    hostEditorOpen = true;
    hostTrustId = hostTrust.host_trust_id;
    hostname = hostTrust.hostname;
    port = hostTrust.port;
    hostKey = hostTrust.host_key;
    hostExpectedRevision = hostTrust.current_revision;
    hostErrors = {};
    hostFormNotice = null;
    await tick();
    hostKeyInput?.focus();
  }

  async function closeHostEditor(restore = true) {
    const returnFocus = hostReturnFocus;
    const shouldRestoreFocus = restore && focusIsInside(hostFormElement);
    const fallbackId = hostExpectedRevision !== null ? `rotate-host-${hostTrustId}` : undefined;
    hostEditorOpen = false;
    hostTrustId = '';
    hostname = '';
    port = 22;
    hostKey = '';
    hostExpectedRevision = null;
    hostErrors = {};
    hostFormNotice = null;
    hostReturnFocus = null;
    if (shouldRestoreFocus) await restoreFocus(returnFocus, fallbackId);
  }

  async function saveHostTrust() {
    hostErrors = validateHostTrust({ hostTrustId, hostname, port, hostKey });
    if (Object.keys(hostErrors).length > 0) {
      await focusFirstInvalid(hostFormElement);
      return;
    }
    const rotating = hostExpectedRevision !== null;
    const rotationTarget = editingHostTrust;
    if (rotating && !rotationTarget) {
      hostFormNotice = { tone: 'error', text: 'This pinned host key changed or was removed. Close the editor and try again.' };
      return;
    }
    const operation = startOperation(rotating ? `rotate-host-${hostTrustId}` : 'add-host-trust');
    if (!operation) return;
    hostFormNotice = null;
    try {
      const body: PutRepositorySshHostTrustRequest = {
        operation_id: operationId('host-trust-save'),
        host_trust_id: hostTrustId.trim(),
        hostname: rotationTarget?.hostname ?? hostname.trim(),
        port: rotationTarget?.port ?? port,
        host_key: hostKey.trim(),
        expected_revision: hostExpectedRevision
      };
      const saved = await request('/host-trusts', 'POST', body, parseRepositorySshHostTrust, rotating ? 'rotate this pinned SSH host key' : 'pin this SSH host key');
      requireCurrentOperation(operation);
      hostTrusts = rotating
        ? hostTrusts.map((entry) => entry.host_trust_id === saved.host_trust_id ? saved : entry)
        : [...hostTrusts, saved].sort((a, b) => a.host_trust_id.localeCompare(b.host_trust_id));
      finishOperation(operation);
      await closeHostEditor();
      requireCurrentOperation(operation);
      const notice: Notice = { tone: 'success', text: `${rotating ? 'Rotated' : 'Pinned'} the key for ${saved.hostname}:${saved.port}. This records host identity; it does not prove Git authentication succeeded.` };
      if (rotating) hostRowNotices = { ...hostRowNotices, [saved.host_trust_id]: notice };
      else hostNotice = notice;
    } catch (error) {
      if (error instanceof StaleRepositoryAccessRequestError) return;
      hostFormNotice = operationError(error, 'Failed to save the pinned host key.');
    } finally {
      finishOperation(operation);
    }
  }

  async function deleteHostTrust(hostTrust: RepositorySshHostTrust) {
    if (hostTrust.referenced_repositories.length > 0) return;
    const operationName = `delete-host-${hostTrust.host_trust_id}`;
    if (isPending(operationName)) return;
    if (!confirm(`Delete the pinned host key for ${hostTrust.hostname}:${hostTrust.port}? Future SSH connections cannot verify this host until another key is pinned.`)) return;
    const operation = startOperation(operationName);
    if (!operation) return;
    hostRowNotices = { ...hostRowNotices, [hostTrust.host_trust_id]: { tone: 'success', text: 'Deleting pinned key…' } };
    try {
      const body: DeleteRepositorySshHostTrustRequest = {
        operation_id: operationId('host-trust-delete'),
        expected_revision: hostTrust.current_revision
      };
      await request(`/host-trusts/${encodeURIComponent(hostTrust.host_trust_id)}`, 'DELETE', body, null, `delete the pin for ${hostTrust.hostname}:${hostTrust.port}`);
      requireCurrentOperation(operation);
      hostTrusts = hostTrusts.filter((entry) => entry.host_trust_id !== hostTrust.host_trust_id);
      hostNotice = { tone: 'success', text: `Deleted the pin for ${hostTrust.hostname}:${hostTrust.port}.` };
    } catch (error) {
      if (error instanceof StaleRepositoryAccessRequestError) return;
      hostRowNotices = { ...hostRowNotices, [hostTrust.host_trust_id]: operationError(error, 'Host key deletion failed.') };
    } finally {
      finishOperation(operation);
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
    {#if data.accessProjectionError}
      <div class="repository-access-empty error" role="alert">
        <strong>Repository bindings unavailable</strong>
        <p>{data.accessProjectionError}</p>
        <button type="button" class="secondary" onclick={() => window.location.reload()}>Retry bindings</button>
      </div>
    {:else if accessProjection && accessProjection.bindings.length === 0}
      <div class="repository-access-empty">
        <strong>No explicit bindings</strong>
        <p>SSH Repositories can still use the Workspace default credential with normal read/write Git access and a unique pinned key that matches their host and port.</p>
      </div>
    {:else if accessProjection}
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
    {#if accessProjection}
      <details class="repository-access-technical">
        <summary>Technical details</summary>
        <dl>
          <div><dt>Config revision</dt><dd>{accessProjection.config_revision}</dd></div>
          <div><dt>Projection digest</dt><dd><code>{accessProjection.projection_digest}</code></dd></div>
        </dl>
      </details>
    {/if}
  </section>

  <section class="repository-access-section" aria-labelledby="credentials-heading">
    <div class="repository-access-section-heading">
      <div>
        <h2 id="credentials-heading">SSH credentials</h2>
        <p>Keys this Workspace can offer when it authenticates to an SSH Repository.</p>
      </div>
      {#if !data.credentialsError}
        <div class="repository-access-actions" aria-label="Add SSH credential">
          <button type="button" class="secondary" disabled={credentialEditorPending()} onclick={() => void openCredentialForm('generate')}>Generate credential</button>
          <button type="button" class="secondary" disabled={credentialEditorPending()} onclick={() => void openCredentialForm('import')}>Import credential</button>
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
          <thead><tr><th>Credential</th><th>Role</th><th>Fingerprint</th><th>Repository application</th><th><span class="visually-hidden">Actions</span></th></tr></thead>
          <tbody>
            {#each credentials as credential (credential.credential_id)}
              <tr aria-busy={credentialLifecyclePending(credential.credential_id)}>
                <td><strong>{credential.name}</strong><code>{credential.credential_id}</code></td>
                <td>{credential.credential_id === workspaceDefaultCredentialId ? 'Workspace default' : 'Additional'}</td>
                <td><code>{credential.public_key_algorithm}</code><code>{credential.public_key_fingerprint}</code></td>
                <td>
                  {#if credential.credential_id === workspaceDefaultCredentialId}
                    Automatic for all SSH Repositories
                  {:else if credential.referenced_repositories.length > 0}
                    Explicit: {credential.referenced_repositories.join(', ')}
                  {:else}
                    No explicit references
                  {/if}
                </td>
                <td>
                  <div class="repository-access-row-actions">
                    <button type="button" class="secondary" disabled={credentialLifecyclePending(credential.credential_id)} onclick={() => void copyPublicKey(credential)}>
                      {isPending(`copy-${credential.credential_id}`) ? 'Loading…' : copiedCredentialId === credential.credential_id ? 'Copied' : 'Copy public key'}
                    </button>
                    {#if credential.credential_id !== workspaceDefaultCredentialId}
                      <button id={`rotate-credential-${credential.credential_id}`} type="button" class="secondary" disabled={credentialEditorPending() || credentialLifecyclePending(credential.credential_id)} onclick={() => void openCredentialRotation(credential)}>Rotate</button>
                      {#if credential.referenced_repositories.length === 0}
                        <button type="button" class="danger" disabled={credentialLifecyclePending(credential.credential_id)} onclick={() => void deleteCredential(credential)}>Delete</button>
                      {/if}
                    {/if}
                  </div>
                  {#if credentialRowNotices[credential.credential_id]}
                    <small class:field-error={credentialRowNotices[credential.credential_id].tone === 'error'} role={credentialRowNotices[credential.credential_id].tone === 'error' ? 'alert' : 'status'}>{credentialRowNotices[credential.credential_id].text}</small>
                  {/if}
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
      <form bind:this={credentialFormElement} class="repository-access-form" aria-busy={isPending('generate-credential')} novalidate onsubmit={(event) => { event.preventDefault(); void generateCredential(); }}>
        <div class="repository-access-form-heading"><div><h3>Generate SSH credential</h3><p>Creates an Ed25519 key. Copy the public key to the Repository provider after saving.</p></div></div>
        {#if credentialFormNotice}<p class="repository-access-notice {credentialFormNotice.tone}" role={credentialFormNotice.tone === 'error' ? 'alert' : 'status'}>{credentialFormNotice.text}</p>{/if}
        <div class="repository-access-form-grid">
          <label><span>Credential id</span><input bind:this={generateCredentialIdInput} bind:value={generateCredentialId} aria-invalid={Boolean(generateErrors.credentialId)} aria-describedby={generateErrors.credentialId ? 'generate-id-error' : undefined} autocomplete="off" />{#if generateErrors.credentialId}<small id="generate-id-error" class="field-error">{generateErrors.credentialId}</small>{/if}</label>
          <label><span>Name</span><input bind:value={generateCredentialName} aria-invalid={Boolean(generateErrors.name)} aria-describedby={generateErrors.name ? 'generate-name-error' : undefined} autocomplete="off" />{#if generateErrors.name}<small id="generate-name-error" class="field-error">{generateErrors.name}</small>{/if}</label>
        </div>
        <div class="repository-access-form-actions"><button type="submit" class="primary" disabled={isPending('generate-credential')}>{isPending('generate-credential') ? 'Generating…' : 'Generate credential'}</button><button type="button" class="secondary" disabled={isPending('generate-credential')} onclick={() => void closeCredentialForms()}>Cancel</button></div>
      </form>
    {:else if credentialForm === 'import'}
      <form bind:this={credentialFormElement} class="repository-access-form" aria-busy={isPending('import-credential')} novalidate onsubmit={(event) => { event.preventDefault(); void createCredential(); }}>
        <div class="repository-access-form-heading"><div><h3>Import SSH credential</h3><p>The private key and optional passphrase are write-only and cleared after every save attempt.</p></div></div>
        {#if credentialFormNotice}<p class="repository-access-notice {credentialFormNotice.tone}" role={credentialFormNotice.tone === 'error' ? 'alert' : 'status'}>{credentialFormNotice.text}</p>{/if}
        <div class="repository-access-form-grid">
          <label><span>Credential id</span><input bind:this={importCredentialIdInput} bind:value={credentialId} aria-invalid={Boolean(importErrors.credentialId)} aria-describedby={importErrors.credentialId ? 'import-id-error' : undefined} autocomplete="off" />{#if importErrors.credentialId}<small id="import-id-error" class="field-error">{importErrors.credentialId}</small>{/if}</label>
          <label><span>Name</span><input bind:value={credentialName} aria-invalid={Boolean(importErrors.name)} aria-describedby={importErrors.name ? 'import-name-error' : undefined} autocomplete="off" />{#if importErrors.name}<small id="import-name-error" class="field-error">{importErrors.name}</small>{/if}</label>
          <label class="wide"><span>OpenSSH private key (Ed25519)</span><textarea bind:value={privateKey} rows="8" autocomplete="off" data-web-ux-redact aria-invalid={Boolean(importErrors.privateKey)} aria-describedby={importErrors.privateKey ? 'import-key-error' : 'import-key-help'}></textarea><small id="import-key-help">Secret value; it cannot be read back after import.</small>{#if importErrors.privateKey}<small id="import-key-error" class="field-error">{importErrors.privateKey}</small>{/if}</label>
          <label class="wide"><span>Passphrase (optional)</span><input type="password" bind:value={passphrase} autocomplete="new-password" data-web-ux-redact /><small>Enter only when the private key is encrypted.</small></label>
        </div>
        <div class="repository-access-form-actions"><button type="submit" class="primary" disabled={isPending('import-credential')}>{isPending('import-credential') ? 'Importing…' : 'Import credential'}</button><button type="button" class="secondary" disabled={isPending('import-credential')} onclick={() => void closeCredentialForms()}>Cancel</button></div>
      </form>
    {/if}

    {#if selectedCredential}
      <form bind:this={rotationFormElement} class="repository-access-form" aria-busy={isPending(`rotate-${selectedCredential.credential_id}`)} novalidate onsubmit={(event) => { event.preventDefault(); void rotateCredential(selectedCredential); }}>
        <div class="repository-access-form-heading">
          <div><h3>Rotate {selectedCredential.name}</h3><p>{selectedCredential.referenced_repositories.length > 0 ? `Explicitly referenced by ${selectedCredential.referenced_repositories.join(', ')}. ` : 'No Repository explicitly references this credential. '}Every external provider where the current public key is installed must be updated after rotation.</p></div>
          <code>{selectedCredential.public_key_fingerprint}</code>
        </div>
        {#if credentialFormNotice}<p class="repository-access-notice {credentialFormNotice.tone}" role={credentialFormNotice.tone === 'error' ? 'alert' : 'status'}>{credentialFormNotice.text}</p>{/if}
        <div class="repository-access-form-grid">
          <label class="wide"><span>New OpenSSH private key (Ed25519)</span><textarea bind:this={rotatePrivateKeyInput} bind:value={rotatePrivateKey} rows="8" autocomplete="off" data-web-ux-redact aria-invalid={Boolean(rotateErrors.privateKey)} aria-describedby={rotateErrors.privateKey ? 'rotate-key-error' : 'rotate-key-help'}></textarea><small id="rotate-key-help">Secret value; it cannot be read back after rotation.</small>{#if rotateErrors.privateKey}<small id="rotate-key-error" class="field-error">{rotateErrors.privateKey}</small>{/if}</label>
          <label class="wide"><span>Passphrase (optional)</span><input type="password" bind:value={rotatePassphrase} autocomplete="new-password" data-web-ux-redact /></label>
        </div>
        <div class="repository-access-form-actions"><button type="submit" class="primary" disabled={credentialLifecyclePending(selectedCredential.credential_id)}>{isPending(`rotate-${selectedCredential.credential_id}`) ? 'Rotating…' : 'Rotate credential'}</button><button type="button" class="secondary" disabled={isPending(`rotate-${selectedCredential.credential_id}`)} onclick={() => void closeCredentialRotation()}>Cancel</button></div>
      </form>
    {/if}

    {#if credentials.length > 0 && !data.credentialsError}
      <details class="repository-access-technical">
        <summary>Credential revisions</summary>
        <dl>{#each credentials as credential (credential.credential_id)}<div><dt>{credential.credential_id}</dt><dd>Revision {credential.current_revision}</dd></div>{/each}</dl>
      </details>
    {/if}
  </section>

  <section class="repository-access-section" aria-labelledby="host-trust-heading">
    <div class="repository-access-section-heading">
      <div>
        <h2 id="host-trust-heading">Pinned SSH host keys</h2>
        <p>Keys used to verify the identity of a specific SSH host and port.</p>
      </div>
      {#if !data.hostTrustsError}<button type="button" class="secondary" disabled={hostEditorPending()} onclick={() => void openNewHostTrust()}>Add pinned key</button>{/if}
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
          <thead><tr><th>Host and port</th><th>Fingerprint</th><th>Repository application</th><th><span class="visually-hidden">Actions</span></th></tr></thead>
          <tbody>
            {#each hostTrusts as hostTrust (hostTrust.host_trust_id)}
              <tr aria-busy={isPending(`rotate-host-${hostTrust.host_trust_id}`) || isPending(`delete-host-${hostTrust.host_trust_id}`)}>
                <td><strong>{hostTrust.hostname}:{hostTrust.port}</strong><code>{hostTrust.host_trust_id}</code></td>
                <td><code>{hostTrust.key_algorithm}</code><code>{hostTrust.fingerprint}</code></td>
                <td>{hostTrust.referenced_repositories.length > 0 ? `Explicit: ${hostTrust.referenced_repositories.join(', ')}` : 'No explicit references; may apply automatically when this is the unique pin for a matching host and port.'}</td>
                <td>
                  <div class="repository-access-row-actions">
                    <button id={`rotate-host-${hostTrust.host_trust_id}`} type="button" class="secondary" disabled={hostEditorPending() || isPending(`delete-host-${hostTrust.host_trust_id}`)} onclick={() => void editHostTrust(hostTrust)}>Rotate key</button>
                    {#if hostTrust.referenced_repositories.length === 0}<button type="button" class="danger" disabled={isPending(`delete-host-${hostTrust.host_trust_id}`) || isPending(`rotate-host-${hostTrust.host_trust_id}`)} onclick={() => void deleteHostTrust(hostTrust)}>Delete</button>{/if}
                  </div>
                  {#if hostRowNotices[hostTrust.host_trust_id]}
                    <small class:field-error={hostRowNotices[hostTrust.host_trust_id].tone === 'error'} role={hostRowNotices[hostTrust.host_trust_id].tone === 'error' ? 'alert' : 'status'}>{hostRowNotices[hostTrust.host_trust_id].text}</small>
                  {:else if hostTrust.referenced_repositories.length > 0}
                    <small>Remove explicit Repository references before deletion.</small>
                  {/if}
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
      <form bind:this={hostFormElement} class="repository-access-form" aria-busy={hostExpectedRevision === null ? isPending('add-host-trust') : isPending(`rotate-host-${hostTrustId}`)} novalidate onsubmit={(event) => { event.preventDefault(); void saveHostTrust(); }}>
        <div class="repository-access-form-heading">
          <div>
            <h3>{hostExpectedRevision === null ? 'Add pinned host key' : `Rotate key for ${hostname}:${port}`}</h3>
            <p>{hostExpectedRevision === null ? 'Confirm the host, port, and fingerprint against a trusted source before saving.' : `This keeps the endpoint fixed and replaces its current trusted identity. ${editingHostTrust?.referenced_repositories.length ? `Explicitly referenced by ${editingHostTrust.referenced_repositories.join(', ')}. ` : 'No Repository explicitly references this pin. '}Repositories without an explicit host-key binding may also select it automatically when it is the unique pin matching this host and port.`}</p>
          </div>
          {#if hostExpectedRevision !== null}<div><small>Current fingerprint</small><code>{editingHostTrust?.fingerprint}</code></div>{/if}
        </div>
        {#if hostFormNotice}<p class="repository-access-notice {hostFormNotice.tone}" role={hostFormNotice.tone === 'error' ? 'alert' : 'status'}>{hostFormNotice.text}</p>{/if}
        <div class="repository-access-form-grid">
          <label><span>Host trust id</span><input bind:this={hostTrustIdInput} bind:value={hostTrustId} disabled={hostExpectedRevision !== null} aria-invalid={Boolean(hostErrors.hostTrustId)} aria-describedby={hostErrors.hostTrustId ? 'host-id-error' : undefined} />{#if hostErrors.hostTrustId}<small id="host-id-error" class="field-error">{hostErrors.hostTrustId}</small>{/if}</label>
          <label><span>Hostname</span><input bind:value={hostname} disabled={hostExpectedRevision !== null} aria-invalid={Boolean(hostErrors.hostname)} aria-describedby={hostErrors.hostname ? 'hostname-error' : undefined} />{#if hostErrors.hostname}<small id="hostname-error" class="field-error">{hostErrors.hostname}</small>{/if}</label>
          <label><span>Port</span><input type="number" bind:value={port} disabled={hostExpectedRevision !== null} aria-invalid={Boolean(hostErrors.port)} aria-describedby={hostErrors.port ? 'host-port-error' : undefined} />{#if hostErrors.port}<small id="host-port-error" class="field-error">{hostErrors.port}</small>{/if}</label>
          <label class="wide"><span>OpenSSH public host key (Ed25519)</span><textarea bind:this={hostKeyInput} bind:value={hostKey} rows="4" aria-invalid={Boolean(hostErrors.hostKey)} aria-describedby={hostErrors.hostKey ? 'host-key-error host-key-help' : 'host-key-help'}></textarea><small id="host-key-help">The new fingerprint is derived from this key after save; compare the key with a trusted source first.</small>{#if hostErrors.hostKey}<small id="host-key-error" class="field-error">{hostErrors.hostKey}</small>{/if}</label>
        </div>
        <div class="repository-access-form-actions"><button type="submit" class="primary" disabled={hostExpectedRevision === null ? isPending('add-host-trust') : isPending(`rotate-host-${hostTrustId}`)}>{hostExpectedRevision === null ? (isPending('add-host-trust') ? 'Saving…' : 'Pin host key') : (isPending(`rotate-host-${hostTrustId}`) ? 'Saving…' : 'Save new key')}</button><button type="button" class="secondary" disabled={hostExpectedRevision === null ? isPending('add-host-trust') : isPending(`rotate-host-${hostTrustId}`)} onclick={() => void closeHostEditor()}>Cancel</button></div>
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
