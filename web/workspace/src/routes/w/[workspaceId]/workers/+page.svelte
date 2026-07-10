<script lang="ts">
  import { workerConsoleHref } from '$lib/workspace-console/model';
  import type { Worker } from '$lib/workspace-sidebar/types';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();

  function workerStatus(worker: Worker): string {
    return `${worker.state} · ${worker.status}`;
  }

  function workerProfile(worker: Worker): string {
    return worker.profile ?? worker.role ?? 'unknown';
  }

  function workerDirectory(worker: Worker): string {
    const directory = worker.working_directory;
    if (!directory) return '—';
    const selector = directory.requested_selector ?? 'HEAD';
    const commit = directory.resolved_commit ? directory.resolved_commit.slice(0, 12) : null;
    return commit ? `${directory.repository_id} · ${selector} · ${commit}` : `${directory.repository_id} · ${selector}`;
  }
</script>

<svelte:head>
  <title>Workers · Yoi Workspace</title>
  <meta name="description" content="Workspace Workers" />
</svelte:head>

<section class="workers-page" aria-labelledby="workers-heading">
  <header class="workers-page-header">
    <div>
      <h1 id="workers-heading">Workers</h1>
      <p>Workers running or persisted for this workspace.</p>
    </div>
    <a class="section-action" href={`/w/${data.workspaceId}/workers/new`}>New Worker</a>
  </header>

  {#if data.workersError}
    <p class="section-state error">{data.workersError}</p>
  {:else if !data.workers}
    <p class="section-state">Loading Workers…</p>
  {:else if data.workers.items.length === 0}
    <p class="section-state">No Workers are visible.</p>
  {:else}
    <div class="table-wrap workers-table-wrap">
      <table class="workers-table">
        <thead>
          <tr>
            <th>Worker</th>
            <th>Runtime</th>
            <th>Profile</th>
            <th>Status</th>
            <th>Working directory</th>
            <th>Action</th>
          </tr>
        </thead>
        <tbody>
          {#each data.workers.items as worker}
            <tr>
              <td>
                <strong>{worker.label}</strong>
                <small><code>{worker.worker_id}</code></small>
              </td>
              <td><code>{worker.runtime_id}</code></td>
              <td>{workerProfile(worker)}</td>
              <td>{workerStatus(worker)}</td>
              <td>{workerDirectory(worker)}</td>
              <td>
                <a class="inline-link" href={workerConsoleHref(worker, data.workspaceId)}>Open Console</a>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</section>
