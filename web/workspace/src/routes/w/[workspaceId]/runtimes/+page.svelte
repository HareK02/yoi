<script lang="ts">
  import type { Runtime } from '$lib/workspace/sidebar/types';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();

  function runtimePlatform(runtime: Runtime): string {
    return `${runtime.capabilities.os} / ${runtime.capabilities.arch}`;
  }
</script>

<svelte:head>
  <title>Runtimes · Yoi Workspace</title>
  <meta name="description" content="Workspace Runtimes" />
</svelte:head>

<section class="runtimes-page" aria-labelledby="runtimes-heading">
  <header class="page-header-row">
    <div>
      <h1 id="runtimes-heading">Runtimes</h1>
      <p>Execution backends available to this workspace.</p>
    </div>
  </header>

  {#if data.runtimesError}
    <p class="section-state error">{data.runtimesError}</p>
  {:else if !data.runtimes}
    <p class="section-state">Loading Runtimes…</p>
  {:else if data.runtimes.items.length === 0}
    <p class="section-state">No Runtimes are visible.</p>
  {:else}
    <div class="table-wrap">
      <table class="runtimes-table">
        <thead>
          <tr>
            <th>Runtime</th>
            <th>Kind</th>
            <th>Status</th>
            <th>Platform</th>
            <th>Capacity</th>
            <th>Workdirs</th>
          </tr>
        </thead>
        <tbody>
          {#each data.runtimes.items as runtime}
            <tr>
              <td>
                <strong>{runtime.label}</strong>
                <small><code>{runtime.runtime_id}</code></small>
              </td>
              <td>{runtime.kind}</td>
              <td>{runtime.status}</td>
              <td>{runtimePlatform(runtime)}</td>
              <td>{runtime.capabilities.max_workers} workers</td>
              <td>
                <a class="inline-link" href={`/w/${data.workspaceId}/runtimes/${encodeURIComponent(runtime.runtime_id)}/workdirs`}>
                  Open workdirs
                </a>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</section>
