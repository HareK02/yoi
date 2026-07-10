<script lang="ts">
  import type { WorkingDirectorySummary } from '$lib/workspace-sidebar/types';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
  let runtimeLabel = $derived(
    data.runtimes?.items.find((runtime) => runtime.runtime_id === data.runtimeId)?.label ?? data.runtimeId,
  );

  function commitLabel(workdir: WorkingDirectorySummary): string {
    return workdir.resolved_commit ? workdir.resolved_commit.slice(0, 12) : '—';
  }

  function selectorLabel(workdir: WorkingDirectorySummary): string {
    return workdir.requested_selector ?? 'HEAD';
  }
</script>

<svelte:head>
  <title>Workdirs · {runtimeLabel} · Yoi Workspace</title>
  <meta name="description" content="Runtime workdirs" />
</svelte:head>

<section class="workdirs-page" aria-labelledby="workdirs-heading">
  <header class="page-header-row">
    <div>
      <p class="breadcrumb"><a href={`/w/${data.workspaceId}/runtimes`}>Runtimes</a> / {runtimeLabel}</p>
      <h1 id="workdirs-heading">Workdirs</h1>
      <p>Workdirs owned by <code>{data.runtimeId}</code>.</p>
    </div>
  </header>

  {#if data.workdirsError}
    <p class="section-state error">{data.workdirsError}</p>
  {:else if !data.workdirs}
    <p class="section-state">Loading workdirs…</p>
  {:else if data.workdirs.items.length === 0}
    <p class="section-state">No workdirs are visible for this Runtime.</p>
  {:else}
    <div class="table-wrap">
      <table class="workdirs-table">
        <thead>
          <tr>
            <th>Workdir</th>
            <th>Repository</th>
            <th>Selector</th>
            <th>Commit</th>
            <th>Status</th>
            <th>Policy</th>
          </tr>
        </thead>
        <tbody>
          {#each data.workdirs.items as workdir}
            <tr>
              <td><code>{workdir.working_directory_id}</code></td>
              <td>{workdir.repository_id}</td>
              <td>{selectorLabel(workdir)}</td>
              <td><code>{commitLabel(workdir)}</code></td>
              <td>{workdir.status}</td>
              <td>
                <span>{workdir.dirty_state_policy}</span>
                <small>{workdir.cleanup_policy}</small>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</section>
