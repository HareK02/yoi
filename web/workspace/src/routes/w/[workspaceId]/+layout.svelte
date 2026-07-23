<script lang="ts">
  import { page } from '$app/state';
  import SidebarOverride from '$lib/workspace/sidebar/SidebarOverride.svelte';
  import WorkspaceSidebar from '$lib/workspace/sidebar/WorkspaceSidebar.svelte';
  import { getSidebarController } from '$lib/workspace/sidebar/context';
  import type { LayoutProps } from './$types';

  let { data, children }: LayoutProps = $props();
  let sidebarCollapsed = $state(false);
  const sidebarController = getSidebarController();

  function toggleSidebar() {
    sidebarCollapsed = !sidebarCollapsed;
  }

  $effect(() => {
    sidebarController.setCollapsed(sidebarCollapsed);
  });
</script>

{#snippet workspaceSidebar()}
  <WorkspaceSidebar
    workspace={data.workspace ?? null}
    workspaceError={data.workspaceError ?? null}
    repositories={data.repositories ?? null}
    repositoriesError={data.repositoriesError ?? null}
    currentPath={page.url.pathname}
    collapsed={sidebarCollapsed}
    onToggleCollapsed={toggleSidebar}
  />
{/snippet}

<SidebarOverride sidebar={workspaceSidebar} />

{@render children()}
