<script lang="ts">
  import ObjectivesNavSection from './ObjectivesNavSection.svelte';
  import RepositoriesNavSection from './RepositoriesNavSection.svelte';
  import WorkersNavSection from './WorkersNavSection.svelte';
  import { workspaceRoute } from '$lib/workspace-api/http';
  import type { RepositoryListResponse, WorkspaceResponse } from './types';

  type Props = {
    workspace: WorkspaceResponse | null;
    workspaceError?: string | null;
    repositories?: RepositoryListResponse | null;
    repositoriesError?: string | null;
    currentPath?: string;
  };

  let {
    workspace,
    workspaceError = null,
    repositories = null,
    repositoriesError = null,
    currentPath = '/'
  }: Props = $props();

  let workspaceId = $derived(workspace?.workspace_id ?? '');
  let settingsHref = $derived(workspaceId ? workspaceRoute(workspaceId, '/settings') : '/settings');
</script>

<aside class="workspace-sidebar" aria-label="Workspace navigation">
  <header class="sidebar-header">
    <div class="workspace-label">
      {#if workspace}
        <p class="workspace-status">{workspace.workspace_id}</p>
        <h1>{workspace.display_name}</h1>
      {:else}
        <h1>Yoi workspace</h1>
        {#if workspaceError}
          <p class="workspace-status error">Workspace summary unavailable.</p>
        {:else}
          <p class="workspace-status">Loading workspace…</p>
        {/if}
      {/if}
    </div>

    <a
      class="settings-button"
      href={settingsHref}
      aria-label="Open Settings / Admin"
      title="Settings / Admin"
    >
      ⚙
    </a>
  </header>

  <nav class="sidebar-sections" aria-label="Workspace sections">
    <RepositoriesNavSection {repositories} {repositoriesError} {currentPath} {workspaceId} />
    <ObjectivesNavSection {currentPath} {workspaceId} />
    <WorkersNavSection {currentPath} {workspaceId} />

  </nav>
</aside>
