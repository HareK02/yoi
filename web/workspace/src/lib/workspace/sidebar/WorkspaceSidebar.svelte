<script lang="ts">
  import type { Snippet } from 'svelte';
  import './sidebar.css';
  import { workspaceRoute } from '$lib/workspace/api/http';
  import ObjectivesNavSection from './ObjectivesNavSection.svelte';
  import MemoryNavSection from './MemoryNavSection.svelte';
  import MergeRequestsNavSection from './MergeRequestsNavSection.svelte';
  import TicketsNavSection from './TicketsNavSection.svelte';
  import WorkersNavSection from './WorkersNavSection.svelte';
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
  let workspaceHomeHref = $derived(workspaceId ? workspaceRoute(workspaceId) : '/');
  let workspaceSettingsHref = $derived(
    workspaceId ? workspaceRoute(workspaceId, '/settings') : '/',
  );
</script>

<div class="workspace-sidebar">
  <header class="sidebar-header">
    {#if workspace}
      <nav class="workspace-sidebar-shortcuts" aria-label="Workspace shortcuts">
          <a
            class="workspace-sidebar-shortcut"
            class:active={currentPath === workspaceHomeHref}
            href={workspaceHomeHref}
            aria-label="Workspace home"
            title="Workspace home"
            aria-current={currentPath === workspaceHomeHref ? 'page' : undefined}
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path d="m3 11 9-8 9 8"></path>
              <path d="M5 10v10h14V10"></path>
              <path d="M9 20v-6h6v6"></path>
            </svg>
          </a>
          <a
            class="workspace-sidebar-shortcut"
            class:active={currentPath.startsWith(workspaceSettingsHref)}
            href={workspaceSettingsHref}
            aria-label="Workspace settings"
            title="Workspace settings"
            aria-current={currentPath.startsWith(workspaceSettingsHref) ? 'page' : undefined}
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path d="M4 5h16M4 12h16M4 19h16"></path>
              <path d="M8 3v4M16 10v4M10 17v4"></path>
            </svg>
        </a>
      </nav>
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
      <MergeRequestsNavSection {currentPath} {workspaceId} />
      <MemoryNavSection {currentPath} {workspaceId} />
      <WorkersNavSection {currentPath} {workspaceId} />
    </nav>
  {/if}
</div>
