<script lang="ts">
  import type {
    ConfirmRepositorySshHostTrustRequest,
    RepositorySshConnectionProbeRequest,
    RepositorySshConnectionProbeResponse
  } from '$lib/generated/workspace-api';
  import { parseRepositorySshHostTrust } from '$lib/workspace/api/repository-access';
  import { formatDate, workspaceApiPath } from '$lib/workspace/api/http';
  import { parseRepositorySshConnectionProbeResponse } from '$lib/workspace/api/workspace-model';
  import { repositorySshProbeRuntimes } from '$lib/workspace/repositories/ssh-connection';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
  let selectedRuntimeId = $state('');
  let probe = $state<RepositorySshConnectionProbeResponse | null>(null);
  let selectedHostKey = $state('');
  let pending = $state(false);
  let connectionMessage = $state<string | null>(null);
  const probeRuntimes = $derived(data.runtimes ? repositorySshProbeRuntimes(data.runtimes.items) : []);

  $effect(() => {
    if (!selectedRuntimeId) {
      selectedRuntimeId = probeRuntimes[0]?.runtime_id ?? '';
    }
  });

  async function requestConnectionTest(method: 'POST' | 'PUT', body: unknown): Promise<unknown> {
    const response = await fetch(
      workspaceApiPath(
        data.repository?.workspace_id ?? '',
        `/repositories/${encodeURIComponent(data.repositoryKey)}/ssh-connection-test`
      ),
      {
        method,
        headers: { accept: 'application/json', 'content-type': 'application/json' },
        body: JSON.stringify(body)
      }
    );
    const value = await response.json();
    if (!response.ok) {
      const record = value && typeof value === 'object' ? value as Record<string, unknown> : null;
      throw new Error(typeof record?.message === 'string' ? record.message : `SSH connection test failed with status ${response.status}`);
    }
    return value;
  }

  async function runConnectionTest() {
    pending = true;
    connectionMessage = null;
    probe = null;
    selectedHostKey = '';
    try {
      const body: RepositorySshConnectionProbeRequest = { runtime_id: selectedRuntimeId };
      probe = parseRepositorySshConnectionProbeResponse(await requestConnectionTest('POST', body));
      selectedHostKey = probe.candidates[0]?.host_key ?? '';
      connectionMessage = probe.trust_state === 'verified'
        ? 'The observed SSH host key matches the Workspace trust record.'
        : 'Review the observed fingerprint before trusting this SSH host.';
    } catch (error) {
      connectionMessage = error instanceof Error ? error.message : 'SSH connection test failed';
    } finally {
      pending = false;
    }
  }

  async function confirmHostTrust() {
    if (!probe || !selectedHostKey) return;
    pending = true;
    connectionMessage = null;
    try {
      const body: ConfirmRepositorySshHostTrustRequest = {
        operation_id: `repository-ssh-confirm-${crypto.randomUUID()}`,
        runtime_id: probe.runtime_id,
        host_key: selectedHostKey,
        expected_host_trust_revision: probe.expected_host_trust_revision
      };
      parseRepositorySshHostTrust(await requestConnectionTest('PUT', body));
      probe = { ...probe, trust_state: 'verified' };
      connectionMessage = 'SSH host trust saved. Future connections must present this key.';
    } catch (error) {
      connectionMessage = error instanceof Error ? error.message : 'Failed to save SSH host trust';
    } finally {
      pending = false;
    }
  }
</script>

<svelte:head>
  <title>{data.repository?.item.repository_key ?? data.repositoryKey} · Repository</title>
</svelte:head>

<section class="card repository-detail-card">
  <h2>Repository</h2>
  {#if data.repository}
    <div class="repository-detail-heading">
      <div>
        <h3>{data.repository.item.repository_key}</h3>
      </div>
      <span class="status-pill" class:warn={data.repository.item.observed_status !== 'ready'}>{data.repository.item.observed_status}</span>
    </div>
    <dl>
      <div>
        <dt>Kind</dt>
        <dd>{data.repository.item.kind}</dd>
      </div>
      <div>
        <dt>Provider</dt>
        <dd>{data.repository.item.provider}</dd>
      </div>
      <div>
        <dt>Source</dt>
        <dd>{data.repository.item.source.kind} · {data.repository.item.source.uri}</dd>
      </div>
      <div>
        <dt>Repository access</dt>
        <dd><a href={`/w/${encodeURIComponent(data.workspace.workspace_id)}/settings/repository-access`}>Manage SSH credentials and pinned host keys</a></dd>
      </div>
      <div>
        <dt>Source revision</dt>
        <dd>{data.repository.item.source_revision} · {data.repository.item.source_fingerprint}</dd>
      </div>
      <div>
        <dt>Observed</dt>
        <dd>{data.repository.item.observed_at ?? 'not observed'}</dd>
      </div>
      <div>
        <dt>Record authority</dt>
        <dd>{data.repository.item.record_authority}</dd>
      </div>
      <div>
        <dt>Default selector</dt>
        <dd>{data.repository.item.default_selector ?? 'none configured'}</dd>
      </div>
      <div>
        <dt>Branch</dt>
        <dd>{data.repository.item.git?.branch ?? 'unknown'}</dd>
      </div>
      <div>
        <dt>HEAD</dt>
        <dd><code>{data.repository.item.git?.head ?? 'unknown'}</code></dd>
      </div>
      <div>
        <dt>Dirty</dt>
        <dd>{data.repository.item.git?.dirty ? 'yes' : 'no'}</dd>
      </div>
    </dl>
    {#if data.repository.item.diagnostics && data.repository.item.diagnostics.length > 0}
      <ul class="diagnostics" aria-label="Repository diagnostics">
        {#each data.repository.item.diagnostics as diagnostic}
          <li><code>{diagnostic.code}</code>: {diagnostic.message}</li>
        {/each}
      </ul>
    {/if}
  {:else if data.repositoryError}
    <p class="error">{data.repositoryError}</p>
  {:else}
    <p>Loading repository…</p>
  {/if}
</section>

{#if data.repository?.item.source.kind === 'ssh'}
  <section class="card repository-detail-card">
    <h2>SSH connection test</h2>
    <p>Observe the SSH host key from the same Runtime that will clone this Repository. Nothing is trusted until you confirm a fingerprint below.</p>
    {#if data.runtimesError}
      <p class="section-state error">{data.runtimesError}</p>
    {:else if data.runtimes}
      <label>
        <span>Runtime</span>
        <select bind:value={selectedRuntimeId} disabled={pending}>
          {#each probeRuntimes as runtime}
            <option value={runtime.runtime_id}>{runtime.label} · {runtime.runtime_id}</option>
          {/each}
        </select>
      </label>
      {#if probeRuntimes.length === 0}
        <p class="section-state error">No configured remote Runtime is available for this connection test.</p>
      {/if}
      <button type="button" disabled={pending || !selectedRuntimeId} onclick={() => void runConnectionTest()}>
        {pending ? 'Checking…' : 'Check SSH connection'}
      </button>
    {/if}

    {#if probe}
      <p><strong>{probe.hostname}:{probe.port}</strong> · {probe.trust_state}</p>
      {#each probe.candidates as candidate}
        <label class="repository-host-key-candidate">
          <input type="radio" name="repository-host-key" bind:group={selectedHostKey} value={candidate.host_key} />
          <span><code>{candidate.algorithm}</code> <code>{candidate.fingerprint}</code></span>
        </label>
      {/each}
      {#if probe.trust_state !== 'verified'}
        <button type="button" class="danger" disabled={pending || !selectedHostKey} onclick={() => void confirmHostTrust()}>
          Confirm and trust selected host key
        </button>
      {/if}
    {/if}
    {#if connectionMessage}<p class="section-state" class:error={probe === null}>{connectionMessage}</p>{/if}
  </section>
{/if}

<section class="card repository-log-card">
  <h2>Recent commits</h2>
  {#if data.repositoryLog}
    {#if data.repositoryLog.items.length === 0}
      <p>No recent commits are available.</p>
    {:else}
      <div class="commit-list">
        {#each data.repositoryLog.items as commit}
          <article class="commit-card">
            <strong>{commit.summary}</strong>
            <span><code>{commit.short_hash}</code> · {commit.author_name} · {formatDate(commit.author_date)}</span>
          </article>
        {/each}
      </div>
    {/if}
    {#if data.repositoryLog.diagnostics.length > 0}
      <ul class="diagnostics" aria-label="Repository log diagnostics">
        {#each data.repositoryLog.diagnostics as diagnostic}
          <li><code>{diagnostic.code}</code>: {diagnostic.message}</li>
        {/each}
      </ul>
    {/if}
  {:else if data.repositoryLogError}
    <p class="error">{data.repositoryLogError}</p>
  {:else}
    <p>Loading repository commits…</p>
  {/if}
</section>
