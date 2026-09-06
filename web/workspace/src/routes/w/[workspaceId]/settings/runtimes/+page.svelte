<script lang="ts">
  import { invalidateAll } from '$app/navigation';
  import type {
    RuntimeConnectionTestResponse,
    WorkspaceRuntimeResource,
  } from '$lib/generated/workspace-api';
  import { testRuntimeConnection } from '$lib/workspace/api/runtime-connection';
  import { workspaceApiPath } from '$lib/workspace/api/http';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
  let runtimeId = $state('');
  let displayName = $state('');
  let endpoint = $state('');
  let showAddRuntime = $state(false);
  let busyRuntimeId = $state<string | null>(null);
  let requestError = $state<string | null>(null);
  let testResults = $state<Record<string, RuntimeConnectionTestResponse>>({});

  function runtimePlatform(runtime: WorkspaceRuntimeResource): string {
    return runtime.os && runtime.arch ? `${runtime.os} / ${runtime.arch}` : 'Unknown';
  }

  function connectionTestSummary(result: RuntimeConnectionTestResponse): string {
    if (result.status === 'compatible') {
      return `Compatible · protocol v${result.actual_protocol_version}`;
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

  async function responseError(response: Response): Promise<string> {
    const payload = await response.json().catch(() => null) as
      | { message?: string; error?: string }
      | null;
    return payload?.message ?? payload?.error ?? `Request failed (${response.status})`;
  }

  async function addRuntime(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    requestError = null;
    busyRuntimeId = 'create';
    try {
      const response = await fetch(workspaceApiPath(data.workspaceId, '/runtimes'), {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          runtime_id: runtimeId,
          display_name: displayName || null,
          endpoint,
        }),
      });
      if (!response.ok) throw new Error(await responseError(response));
      runtimeId = '';
      displayName = '';
      endpoint = '';
      showAddRuntime = false;
      await invalidateAll();
    } catch (error) {
      requestError = error instanceof Error ? error.message : String(error);
    } finally {
      busyRuntimeId = null;
    }
  }

  async function testRuntime(runtime: WorkspaceRuntimeResource): Promise<void> {
    requestError = null;
    busyRuntimeId = runtime.runtime_id;
    try {
      const result = await testRuntimeConnection(data.workspaceId, runtime.runtime_id);
      testResults = { ...testResults, [runtime.runtime_id]: result };
    } catch (error) {
      requestError = error instanceof Error ? error.message : String(error);
    } finally {
      busyRuntimeId = null;
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
      <h2>Add remote Runtime</h2>
      <div class="settings-form-grid">
        <label>
          Runtime ID
          <input bind:value={runtimeId} required autocomplete="off" />
        </label>
        <label>
          Display name
          <input bind:value={displayName} autocomplete="off" />
        </label>
        <label>
          Endpoint
          <input bind:value={endpoint} type="url" required placeholder="https://runtime.example" />
        </label>
      </div>
      <div class="settings-action-row">
        <button type="submit" disabled={busyRuntimeId !== null}>Add Runtime</button>
        <button type="button" disabled={busyRuntimeId !== null} onclick={() => showAddRuntime = false}>
          Cancel
        </button>
      </div>
    </form>
  {/if}

  {#if requestError}
    <p class="section-state error">{requestError}</p>
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
              <td>{runtime.status}</td>
              <td>{runtimePlatform(runtime)}</td>
              <td>{managementLabel(runtime)}</td>
              <td>
                <a class="inline-link" href={`/w/${data.workspaceId}/settings/runtimes/${encodeURIComponent(runtime.runtime_id)}/workdirs`}>
                  Open workdirs
                </a>
              </td>
              <td>
                <div class="settings-action-row">
                  {#if runtime.management?.config_managed}
                    <button
                      type="button"
                      disabled={busyRuntimeId !== null}
                      onclick={() => testRuntime(runtime)}
                    >Test</button>
                  {/if}
                  {#if !runtime.management?.config_managed}
                    <span class="settings-muted-action">Test unavailable</span>
                  {/if}
                </div>
              </td>
            </tr>
            {#if runtime.diagnostics.length > 0 || testResults[runtime.runtime_id]}
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
                  {#if testResults[runtime.runtime_id]}
                    {@const result = testResults[runtime.runtime_id]}
                    <div class:failed={result.status === 'failed'} class="settings-test-result">
                      <strong>Connection test: {connectionTestSummary(result)}</strong>
                      {#if result.diagnostics[0]}
                        <span>{result.diagnostics[0].message}</span>
                      {/if}
                      <small>Checked {new Date(result.checked_at).toLocaleString()}</small>
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
