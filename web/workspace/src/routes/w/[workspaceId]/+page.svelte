<script lang="ts">
  import { workspaceRoute } from '$lib/workspace/api/http';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
  let workspaceId = $derived(data.workspace?.workspace_id ?? data.workspaceId);
  let ticketsHref = $derived(workspaceRoute(workspaceId, '/tickets'));
  let runtimeSettingsHref = $derived(workspaceRoute(workspaceId, '/settings/runtimes'));
  let workersHref = $derived(workspaceRoute(workspaceId, '/workers'));
</script>

<svelte:head>
  <title>Yoi Workspace Control Plane</title>
  <meta name="description" content="Local single-workspace Yoi control plane bootstrap" />
</svelte:head>

<section class="card">
  <h2>Workspace</h2>
  {#if data.workspace}
    <dl>
      <div>
        <dt>ID</dt>
        <dd>{data.workspace.workspace_id}</dd>
      </div>
      <div>
        <dt>Name</dt>
        <dd>{data.workspace.display_name}</dd>
      </div>
      <div>
        <dt>Record authority</dt>
        <dd>{data.workspace.record_authority}</dd>
      </div>
      <div>
        <dt>Host / Worker bridge</dt>
        <dd>{data.workspace.extension_points.host_worker_bridge.status}</dd>
      </div>
    </dl>
  {:else if data.workspaceError}
    <p class="error">{data.workspaceError}</p>
  {:else}
    <p>Waiting for <code>/api/workspace</code>…</p>
  {/if}
</section>

<section class="workspace-actions" aria-label="Workspace sections">
  <a class="workspace-action-card" href={ticketsHref}>
    <span>Tickets</span>
    <strong>Browse workspace tickets</strong>
    <small>Read typed Ticket records</small>
  </a>
  <a class="workspace-action-card" href={runtimeSettingsHref}>
    <span>Runtimes</span>
    <strong>Open admin Runtimes</strong>
    <small>{data.hosts?.items.length ?? 0} host{(data.hosts?.items.length ?? 0) === 1 ? '' : 's'} visible</small>
  </a>
  <a class="workspace-action-card" href={workersHref}>
    <span>Workers</span>
    <strong>Open worker list</strong>
    <small>Inspect status and attach to consoles</small>
  </a>
</section>

<section class="card">
  <h2>Hosts</h2>
  {#if data.hosts}
    {#if data.hosts.items.length === 0}
      <p>No local Hosts are visible.</p>
    {:else}
      <div class="stack">
        {#each data.hosts.items as host}
          <article class="runtime-card">
            <div class="runtime-heading">
              <strong>{host.label}</strong>
              <span class:warn={host.status !== 'available'}>{host.status}</span>
            </div>
            <dl>
              <div>
                <dt>ID</dt>
                <dd><code>{host.host_id}</code></dd>
              </div>
              <div>
                <dt>Kind</dt>
                <dd>{host.kind}</dd>
              </div>
              <div>
                <dt>Runtime</dt>
                <dd><code>{host.runtime_id}</code></dd>
              </div>
              <div>
                <dt>Platform</dt>
                <dd>{host.os} / {host.arch}</dd>
              </div>
            </dl>
          </article>
        {/each}
      </div>
    {/if}
  {:else if data.hostsError}
    <p class="error">{data.hostsError}</p>
  {:else}
    <p>Waiting for <code>/api/hosts</code>…</p>
  {/if}
</section>
