<script lang="ts">
  import { workerConsoleHref } from '$lib/workspace-console/model';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
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

<section class="grid runtime">
  <div class="card">
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
                  <dt>Scope</dt>
                  <dd>{host.capabilities.workspace_scope}</dd>
                </div>
                <div>
                  <dt>Platform</dt>
                  <dd>{host.capabilities.os} / {host.capabilities.arch}</dd>
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
  </div>

  <div class="card">
    <h2>Workers</h2>
    {#if data.workers}
      {#if data.workers.items.length === 0}
        <p>No local Workers are visible.</p>
      {:else}
        <div class="table-wrap">
          <table>
            <thead>
              <tr>
                <th>Worker</th>
                <th>Host</th>
                <th>State</th>
                <th>Workspace</th>
                <th>Implementation</th>
                <th>Attach</th>
              </tr>
            </thead>
            <tbody>
              {#each data.workers.items as worker}
                <tr>
                  <td>
                    <strong>{worker.label}</strong>
                    {#if worker.role || worker.profile}
                      <small>{worker.role ?? 'role unknown'} / {worker.profile ?? 'profile unknown'}</small>
                    {/if}
                  </td>
                  <td><code>{worker.host_id}</code></td>
                  <td>{worker.state} · {worker.status}</td>
                  <td>{worker.workspace.visibility} · {worker.workspace.identity}</td>
                  <td>{worker.implementation.kind}</td>
                  <td><a class="inline-link" href={workerConsoleHref(worker)}>Open Console</a></td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    {:else if data.workersError}
      <p class="error">{data.workersError}</p>
    {:else}
      <p>Waiting for <code>/api/workers</code>…</p>
    {/if}
  </div>
</section>
