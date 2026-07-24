<script lang="ts">
  import { workspaceRoute } from '$lib/workspace/api/http';
  import './sidebar.css';
  import ObjectivesNavSection from './ObjectivesNavSection.svelte';
  import MemoryNavSection from './MemoryNavSection.svelte';
  import RepositoriesNavSection from './RepositoriesNavSection.svelte';
  import TicketsNavSection from './TicketsNavSection.svelte';
  import WorkersNavSection from './WorkersNavSection.svelte';
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
  let homeHref = $derived(workspaceId ? workspaceRoute(workspaceId) : '/');
  let settingsHref = $derived(workspaceId ? workspaceRoute(workspaceId, '/settings') : '/settings');
</script>

<div class="workspace-sidebar">
  <header class="sidebar-header">
    <div class="sidebar-title-row">
      <div class="workspace-label">
        {#if workspace}
          <div class="workspace-name">{workspace.display_name}</div>
        {:else}
          <div class="workspace-name">Yoi workspace</div>
          {#if workspaceError}
            <p class="workspace-status error">Workspace summary unavailable.</p>
          {:else}
            <p class="workspace-status">Loading workspace…</p>
          {/if}
        {/if}
      </div>
    </div>

    <div class="sidebar-actions-row">
      <a
        class="sidebar-icon-button"
        href={homeHref}
        aria-label="Open workspace overview"
        title="Workspace overview"
      >
        <svg class="sidebar-icon" aria-hidden="true" viewBox="0 0 24 24">
          <path d="M15 21v-8a1 1 0 0 0-1-1h-4a1 1 0 0 0-1 1v8" />
          <path
            d="M3 10a2 2 0 0 1 .709-1.528l7-6a2 2 0 0 1 2.582 0l7 6A2 2 0 0 1 21 10v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"
          />
        </svg>
      </a>
      <a
        class="sidebar-icon-button"
        href={settingsHref}
        aria-label="Open Settings / Admin"
        title="Settings / Admin"
      >
        <svg class="sidebar-icon" aria-hidden="true" viewBox="0 0 24 24">
          <path
            d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z"
          />
          <circle cx="12" cy="12" r="4" />
        </svg>
      </a>
    </div>
  </header>

  <nav class="sidebar-sections" aria-label="Workspace sections">
    <RepositoriesNavSection {repositories} {repositoriesError} {currentPath} {workspaceId} />
    <ObjectivesNavSection {currentPath} {workspaceId} />
    <MemoryNavSection {currentPath} {workspaceId} />
    <TicketsNavSection {currentPath} {workspaceId} />
    <WorkersNavSection {currentPath} {workspaceId} />
  </nav>
</div>
