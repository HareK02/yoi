<script lang="ts">
  import type { Runtime } from '$lib/workspace/sidebar/types';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();

  function runtimePlatform(runtime: Runtime): string {
    return `${runtime.os} / ${runtime.arch}`;
  }
</script>

<svelte:head>
  <title>Runtime Inventory · Settings · Yoi Workspace</title>
  <meta name="description" content="Workspace Runtime inventory" />
</svelte:head>

<section class="runtimes-page" aria-labelledby="runtimes-heading">
  <header class="page-header-row">
    <div>
      <h1 id="runtimes-heading">Runtime Inventory</h1>
      <p>Admin view of execution backends available to this workspace.</p>
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
              <td>
                <a class="inline-link" href={`/w/${data.workspaceId}/settings/runtimes/${encodeURIComponent(runtime.runtime_id)}/workdirs`}>
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
