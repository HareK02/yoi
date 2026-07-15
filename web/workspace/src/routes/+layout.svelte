<script lang="ts">
  import { page } from '$app/state';
  import WorkspaceAlerts from '$lib/workspace/alerts/WorkspaceAlerts.svelte';
  import WorkspaceSidebar from '$lib/workspace/sidebar/WorkspaceSidebar.svelte';
  import '../app.css';
  import type { LayoutProps } from './$types';

  let { data, children }: LayoutProps = $props();
  let sidebarCollapsed = $state(false);
</script>

<WorkspaceAlerts />

<div class:sidebar-collapsed={sidebarCollapsed} class="workspace-layout">
  <WorkspaceSidebar
    workspace={data.workspace}
    workspaceError={data.workspaceError}
    repositories={data.repositories}
    repositoriesError={data.repositoriesError}
    currentPath={page.url.pathname}
    collapsed={sidebarCollapsed}
    onToggleCollapsed={() => (sidebarCollapsed = !sidebarCollapsed)}
  />
  <main class="shell">
    {@render children()}
  </main>
</div>
