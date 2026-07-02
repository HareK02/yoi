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
  let settingsActive = $derived(currentPath.startsWith("/settings"));
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
      class:active={settingsActive}
      href="/settings"
      aria-label="Open Settings / Admin"
      title="Settings / Admin"
      aria-current={settingsActive ? 'page' : undefined}
    >
      ⚙
    </a>
  </header>

  <nav class="sidebar-sections" aria-label="Workspace sections">
    <RepositoriesNavSection {workspace} {currentPath} />
    <ObjectivesNavSection {currentPath} />
    <WorkersNavSection {currentPath} />

    <section class="nav-section" aria-labelledby="settings-heading">
      <div class="section-heading-row">
        <h2 id="settings-heading">settings</h2>
      </div>
      <ul class="nav-list" aria-label="Settings">
        <li>
          <a class="nav-item" class:active={settingsActive} href="/settings" aria-current={settingsActive ? 'page' : undefined}>
            <span class="item-title">Settings / Admin</span>
            <span class="item-meta">Backend shell and diagnostics</span>
          </a>
        </li>
      </ul>
    </section>
  </nav>
</aside>
