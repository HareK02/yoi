<script lang="ts">
  import type { Snippet } from 'svelte';
  import './sidebar.css';
  import ObjectivesNavSection from './ObjectivesNavSection.svelte';
  import MemoryNavSection from './MemoryNavSection.svelte';
  import MergeRequestsNavSection from './MergeRequestsNavSection.svelte';
  import TicketsNavSection from './TicketsNavSection.svelte';
  import WorkersNavSection from './WorkersNavSection.svelte';
  import WorkspaceSwitcher from './WorkspaceSwitcher.svelte';
  import type { WorkspaceResponse } from './types';

  type Props = {
    workspace: WorkspaceResponse | null;
    workspaceError?: string | null;
    currentPath?: string;
    content?: Snippet | null;
  };

  let {
    workspace,
    workspaceError = null,
    currentPath = '/',
    content = null,
  }: Props = $props();

  let workspaceId = $derived(workspace?.workspace_id ?? '');
</script>

<div class="workspace-sidebar">
  <header class="sidebar-header">
    {#if workspace}
      <WorkspaceSwitcher
        currentWorkspaceId={workspaceId}
        currentWorkspaceName={workspace.display_name}
      />
    {:else}
      <div class="workspace-label">
        <div class="workspace-name">Yoi workspace</div>
        {#if workspaceError}
          <p class="workspace-status error">Workspace summary unavailable.</p>
        {:else}
          <p class="workspace-status">Loading workspace…</p>
        {/if}
      </div>
    {/if}
  </header>

  {#if content}
    {@render content()}
  {:else}
    <nav class="sidebar-sections" aria-label="Workspace sections">
      <TicketsNavSection {currentPath} {workspaceId} />
      <ObjectivesNavSection {currentPath} {workspaceId} />
      <MemoryNavSection {currentPath} {workspaceId} />
      <MergeRequestsNavSection {currentPath} {workspaceId} />
      <WorkersNavSection {currentPath} {workspaceId} />
    </nav>
  {/if}
</div>
