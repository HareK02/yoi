<script lang="ts">
  import { invalidateAll } from '$app/navigation';
  import type {
    RuntimeConnectionTestResponse,
    RuntimePublicIdentityBundle,
    WorkspaceRuntimeResource,
  } from '$lib/generated/workspace-api';
  import {
    createRemoteRuntime,
    previewRuntimePublicKeyFingerprint,
    RuntimeTrustRequestError,
  } from '$lib/workspace/api/runtime-management';
  import { testRuntimeConnection } from '$lib/workspace/api/runtime-connection';
  import { provisionWorkspaceSigningIdentity } from '$lib/workspace/settings/profile-api';
  import type { PageProps } from './$types';

  const runtimeBundlePlaceholder =
    '{"identity_id":"team-runtime","public_key":"yoi-ed25519-pub:v1:..."}';

  let { data }: PageProps = $props();
  let runtimePublicBundle = $state('');
  let displayName = $state('');
  let endpoint = $state('');
  let runtimeFingerprint = $state<string | null>(null);
  let showAddRuntime = $state(false);
  let busyRuntimeId = $state<string | null>(null);
  let requestError = $state<string | null>(null);
  let requestNotice = $state<string | null>(null);
  let testResults = $state<Record<string, RuntimeConnectionTestResponse>>({});
  let connectionTestGeneration = 0;

  function runtimePlatform(runtime: WorkspaceRuntimeResource): string {
    return runtime.os && runtime.arch ? `${runtime.os} / ${runtime.arch}` : 'Unknown';
  }

  function connectionTestSummary(result: RuntimeConnectionTestResponse): string {
    if (result.status === 'compatible') {
      return result.connection_state === 'verified'
        ? `Verified · protocol v${result.actual_protocol_version}`
        : `Compatible · ${result.connection_state} · protocol v${result.actual_protocol_version}`;
    }
    switch (result.failure_kind) {
      case 'authentication': return 'Authentication failed';
      case 'authorization': return 'Permission or Workspace scope rejected';
      case 'network_unreachable': return 'Runtime unreachable';
      case 'timeout': return 'Connection timed out';
      case 'tls_or_transport': return 'TLS or transport failed';
      case 'malformed_response': return 'Runtime returned an invalid ping response';
      case 'protocol_version_mismatch':
        return `Incompatible protocol · expected v${result.expected_protocol_version}, received v${result.actual_protocol_version ?? 'unknown'}`;
      case 'runtime_identity_mismatch': return 'Runtime identity mismatch';
      case 'configuration': return 'Runtime connection test is not configured';
      default: return 'Connection test failed';
    }
  }

  function managementLabel(runtime: WorkspaceRuntimeResource): string {
    if (runtime.management?.built_in) return 'Built-in';
    if (runtime.management?.config_managed) return 'Managed remote';
    return 'Observed';
  }

  function parseRuntimePublicBundle(value: string): RuntimePublicIdentityBundle {
    let parsed: unknown;
    try {
      parsed = JSON.parse(value);
    } catch {
      throw new Error('Runtime public bundle must be valid JSON');
    }
    if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
      throw new Error('Runtime public bundle must be a JSON object');
    }
    const item = parsed as Record<string, unknown>;
    if (
      Object.keys(item).length !== 2 ||
      typeof item.identity_id !== 'string' ||
      item.identity_id.length === 0 ||
      typeof item.public_key !== 'string' ||
      item.public_key.length === 0
    ) {
      throw new Error('Runtime public bundle must contain only identity_id and public_key');
    }
    return { identity_id: item.identity_id, public_key: item.public_key };
  }

  function workspacePublicBundle(): string {
    return data.signingIdentity?.public_bundle
      ? JSON.stringify(data.signingIdentity.public_bundle, null, 2)
      : '';
  }

  function workspaceBundleFilename(): string {
    return `workspace-${data.workspaceId}-public-bundle.json`;
  }

  async function provisionSigningIdentity(): Promise<void> {
    requestError = null;
    requestNotice = null;
    busyRuntimeId = 'provision-workspace-identity';
    try {
      await provisionWorkspaceSigningIdentity(data.workspaceId);
      await invalidateAll();
      requestNotice = 'Workspace identity provisioned. Copy its public bundle to the Runtime host.';
    } catch (error) {
      requestError = error instanceof Error ? error.message : String(error);
    } finally {
      busyRuntimeId = null;
    }
  }

  async function copyWorkspaceBundle(): Promise<void> {
    requestError = null;
    try {
      await navigator.clipboard.writeText(workspacePublicBundle());
    } catch {
      requestError = 'Workspace public bundle could not be copied';
    }
  }

  async function previewRuntimeFingerprint(): Promise<void> {
    requestError = null;
    runtimeFingerprint = null;
    busyRuntimeId = 'preview';
    try {
      const bundle = parseRuntimePublicBundle(runtimePublicBundle);
      runtimeFingerprint = await previewRuntimePublicKeyFingerprint(bundle.public_key);
    } catch (error) {
      requestError = error instanceof Error ? error.message : String(error);
    } finally {
      busyRuntimeId = null;
    }
  }

  async function addRuntime(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    requestError = null;
    requestNotice = null;
    busyRuntimeId = 'create';
    try {
      const publicBundle = parseRuntimePublicBundle(runtimePublicBundle);
      const currentFingerprint = await previewRuntimePublicKeyFingerprint(publicBundle.public_key);
      if (runtimeFingerprint !== currentFingerprint) {
        throw new Error('Preview the Runtime public key fingerprint before registration');
      }
      await createRemoteRuntime(data.workspaceId, {
        public_bundle: publicBundle,
        display_name: displayName || null,
        endpoint,
        expected_revision: null,
      });
      runtimePublicBundle = '';
      runtimeFingerprint = null;
      displayName = '';
      endpoint = '';
      showAddRuntime = false;
      requestNotice = 'Runtime registered for this Workspace. Run Test to complete authenticated verification.';
      await invalidateAll();
    } catch (error) {
      requestError = error instanceof RuntimeTrustRequestError || error instanceof Error
        ? error.message
        : String(error);
    } finally {
      busyRuntimeId = null;
    }
  }

  function currentTestResult(
    runtime: WorkspaceRuntimeResource,
  ): RuntimeConnectionTestResponse | undefined {
    const result = testResults[runtime.runtime_id];
    return result?.binding_revision === runtime.management?.binding?.revision
      ? result
      : undefined;
  }

  async function testRuntime(runtime: WorkspaceRuntimeResource): Promise<void> {
    const bindingRevision = runtime.management?.binding?.revision;
    if (typeof bindingRevision !== 'number') return;
    const generation = ++connectionTestGeneration;
    requestError = null;
    busyRuntimeId = runtime.runtime_id;
    try {
      const result = await testRuntimeConnection(data.workspaceId, runtime.runtime_id);
      if (
        generation !== connectionTestGeneration ||
        result.binding_revision !== bindingRevision
      ) {
        await invalidateAll();
        return;
      }
      testResults = { ...testResults, [runtime.runtime_id]: result };
      await invalidateAll();
    } catch (error) {
      if (generation === connectionTestGeneration) {
        requestError = error instanceof Error ? error.message : String(error);
      }
    } finally {
      if (generation === connectionTestGeneration) {
        busyRuntimeId = null;
      }
    }
  }
</script>

<svelte:head>
  <title>Runtimes · Settings · Yoi Workspace</title>
  <meta name="description" content="Workspace Runtime resources" />
</svelte:head>

<section class="runtimes-page" aria-labelledby="runtimes-heading">
  <header class="page-header-row">
    <div>
      <h1 id="runtimes-heading">Runtimes</h1>
      <p>Register and inspect the execution backends available to this Workspace.</p>
    </div>
    {#if data.workspace.permissions.manage_runtimes}
      <button type="button" onclick={() => showAddRuntime = !showAddRuntime}>
        {showAddRuntime ? 'Close' : 'Add Runtime'}
      </button>
    {/if}
  </header>

  {#if showAddRuntime && data.workspace.permissions.manage_runtimes}
    <form class="settings-runtime-form" onsubmit={addRuntime}>
      <header>
        <h2>Connect a remote Runtime</h2>
        <p>
          This creates a binding for this Workspace. The Runtime can remain connected to other Workspaces;
          their trust entries are not replaced.
        </p>
      </header>

      <section class="settings-runtime-trust-instructions" aria-labelledby="workspace-to-runtime-heading">
        <h3 id="workspace-to-runtime-heading">1. Trust this Workspace on the Runtime</h3>
        <p>
          Each Workspace has its own signing identity. Add this Workspace public bundle to the same store used
          when starting the Runtime.
        </p>
        {#if data.signingIdentityError}
          <p class="section-state error">{data.signingIdentityError}</p>
        {:else if data.signingIdentity?.identity.state === 'pending_provisioning'}
          <p>This Workspace does not have an active signing identity yet.</p>
          <button
            type="button"
            disabled={busyRuntimeId !== null}
            onclick={() => void provisionSigningIdentity()}
          >
            {busyRuntimeId === 'provision-workspace-identity' ? 'Provisioning…' : 'Provision Workspace identity'}
          </button>
        {:else if data.signingIdentity?.public_bundle}
          <p>
            Save the bundle as <code>{workspaceBundleFilename()}</code> on the Runtime host. It contains no
            private key material.
          </p>
          <pre>{workspacePublicBundle()}</pre>
          <button type="button" disabled={busyRuntimeId !== null} onclick={copyWorkspaceBundle}>
            Copy Workspace public bundle
          </button>
          <pre>yoi-runtime trust-workspace add --bundle {workspaceBundleFilename()}</pre>
          <small>
            Pass the same <code>--fs-root</code> and <code>--fs-runtime-dir</code> options used by the Runtime
            service. Existing Workspace trust entries are preserved.
          </small>
        {:else}
          <p class="section-state">Loading Workspace public identity…</p>
        {/if}
      </section>

      <section class="settings-runtime-trust-instructions" aria-labelledby="runtime-to-workspace-heading">
        <h3 id="runtime-to-workspace-heading">2. Verify the Runtime identity</h3>
        <p>
          On the Runtime host, run <code>yoi-runtime identity show --json</code> with the same Runtime storage
          options, then paste the public bundle below.
        </p>
        <div class="settings-form-grid">
          <label class="settings-form-wide">
            Runtime public bundle
            <textarea
              bind:value={runtimePublicBundle}
              oninput={() => {
                runtimeFingerprint = null;
              }}
              required
              rows="5"
              spellcheck="false"
              placeholder={runtimeBundlePlaceholder}
            ></textarea>
            <button type="button" disabled={busyRuntimeId !== null} onclick={previewRuntimeFingerprint}>
              Preview fingerprint
            </button>
          </label>
          {#if runtimeFingerprint}
            <dl class="runtime-facts">
              <div>
                <dt>Runtime fingerprint</dt>
                <dd><code>{runtimeFingerprint}</code></dd>
              </div>
            </dl>
          {/if}
        </div>
      </section>

      <section class="settings-runtime-trust-instructions" aria-labelledby="runtime-connection-heading">
        <h3 id="runtime-connection-heading">3. Register the connection</h3>
        <div class="settings-form-grid">
          <label>
            Display name
            <input bind:value={displayName} autocomplete="off" />
          </label>
          <label>
            Endpoint
            <input bind:value={endpoint} type="url" required placeholder="https://runtime.example" />
          </label>
        </div>
        <p>
          Registration stores this Workspace-scoped binding. After it appears in the list, run
          <strong>Test</strong> to complete authenticated verification.
        </p>
      </section>

      <div class="settings-action-row">
        <button
          type="submit"
          disabled={busyRuntimeId !== null || !data.signingIdentity?.public_bundle || !runtimeFingerprint}
        >Register Runtime</button>
        <button type="button" disabled={busyRuntimeId !== null} onclick={() => showAddRuntime = false}>
          Cancel
        </button>
      </div>
    </form>
  {/if}

  {#if requestError}
    <p class="section-state error">{requestError}</p>
  {/if}
  {#if requestNotice}
    <p class="section-state">{requestNotice}</p>
  {/if}

  {#if data.runtimesError}
    <p class="section-state error">{data.runtimesError}</p>
  {:else if !data.runtimes}
    <p class="section-state">Loading Runtimes…</p>
  {:else if data.runtimes.items.length === 0}
    <p class="section-state">No Runtimes are visible.</p>
  {:else}
    <div class="settings-runtime-table-wrap">
      <table class="settings-runtime-table">
        <thead>
          <tr>
            <th>Runtime</th>
            <th>Kind</th>
            <th>Status</th>
            <th>Platform</th>
            <th>Management</th>
            <th>Workdirs</th>
            <th>Actions</th>
          </tr>
        </thead>
        <tbody>
          {#each data.runtimes.items as runtime}
            <tr class:inactive={runtime.status !== 'active'}>
              <td>
                <strong>
                  <a class="inline-link" href={`/w/${encodeURIComponent(data.workspaceId)}/settings/runtimes/${encodeURIComponent(runtime.runtime_id)}`}>
                    {runtime.label}
                  </a>
                </strong>
                <small><code>{runtime.runtime_id}</code></small>
              </td>
              <td>{runtime.kind}</td>
              <td>
                {runtime.management?.binding?.connection_state ?? runtime.status}
              </td>
              <td>{runtimePlatform(runtime)}</td>
              <td>{managementLabel(runtime)}</td>
              <td>
                <a class="inline-link" href={`/w/${data.workspaceId}/settings/runtimes/${encodeURIComponent(runtime.runtime_id)}/workdirs`}>
                  Open workdirs
                </a>
              </td>
              <td>
                <div class="settings-action-row">
                  {#if runtime.management?.config_managed && runtime.management.binding?.connection_state !== 'revoked'}
                    <button
                      type="button"
                      disabled={busyRuntimeId !== null}
                      onclick={() => testRuntime(runtime)}
                    >Test</button>
                  {/if}
                  {#if runtime.management?.binding?.state === 'configured'}
                    <span class="settings-muted-action">Verification required</span>
                  {:else if !runtime.management?.config_managed}
                    <span class="settings-muted-action">Test unavailable</span>
                  {/if}
                </div>
              </td>
            </tr>
            {@const currentResult = currentTestResult(runtime)}
            {#if runtime.diagnostics.length > 0 || currentResult}
              <tr class="settings-runtime-detail-row">
                <td colspan="7">
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
                  {#if currentResult}
                    <div class:failed={currentResult.status === 'failed'} class="settings-test-result">
                      <strong>Connection test: {connectionTestSummary(currentResult)}</strong>
                      {#if currentResult.diagnostics[0]}
                        <span>{currentResult.diagnostics[0].message}</span>
                      {/if}
                      <small>Checked {new Date(currentResult.checked_at).toLocaleString()}</small>
                    </div>
                  {/if}
                </td>
              </tr>
            {/if}
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</section>
