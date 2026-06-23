<script lang="ts">
  import ObjectivesNavSection from './ObjectivesNavSection.svelte';
  import RepositoriesNavSection from './RepositoriesNavSection.svelte';
  import WorkersNavSection from './WorkersNavSection.svelte';
  import type { WorkspaceResponse } from './types';

  type Props = {
    workspace: WorkspaceResponse | null;
    workspaceError?: string | null;
    currentPath?: string;
  };

  let { workspace, workspaceError = null, currentPath = '/' }: Props = $props();
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

    <button
      class="settings-button"
      type="button"
      aria-label="Workspace settings"
      title="Workspace settings placeholder"
      disabled
    >
      ⚙
    </button>
  </header>

  <nav class="sidebar-sections" aria-label="Workspace sections">
    <RepositoriesNavSection {workspace} {currentPath} />
    <ObjectivesNavSection {currentPath} />
    <WorkersNavSection />
  </nav>
</aside>
