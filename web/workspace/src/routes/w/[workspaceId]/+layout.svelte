<script lang="ts">
  import { onDestroy, setContext } from 'svelte';
  import { page } from '$app/state';
  import HeaderOverride from '#lib/workspace/header/HeaderOverride.svelte';
  import WorkspaceBreadcrumbs from '#lib/workspace/header/WorkspaceBreadcrumbs.svelte';
  import SidebarOverride from '#lib/workspace/sidebar/SidebarOverride.svelte';
  import {
    getSidebarController,
    SIDEBAR_CONTEXT,
    type SidebarController,
    type SidebarSnippet,
  } from '#lib/workspace/sidebar/context.ts';
  import { createOverrideStack } from '#lib/workspace/sidebar/override-stack.ts';
  import { ownsRoutePath } from '#lib/workspace/sidebar/route-ownership.ts';
  import { workspaceRoute } from '#lib/workspace/api/http.ts';
  import { disposeWorkspaceMultiplexer } from '#lib/workspace/multiplexer.ts';
  import { disposeWorkspaceWorkersStore } from '#lib/workspace/sidebar/worker-subscription.ts';
  import WorkspaceSidebar from '#lib/workspace/sidebar/WorkspaceSidebar.svelte';
  import '#lib/workspace/styles/workspace-pages.css';
  import '#lib/workspace/styles/tickets.css';
  import '#lib/workspace/styles/workers.css';
  import type { LayoutProps } from './$types';

  let { data, children }: LayoutProps = $props();
  const parentSidebarController = getSidebarController();
  let sidebarContent = $state<SidebarSnippet | null>(null);
  const sidebarContentOverrides = createOverrideStack<SidebarSnippet>((activeContent) => {
    sidebarContent = activeContent;
  });

  setContext<SidebarController>(SIDEBAR_CONTEXT, {
    registerSidebar: sidebarContentOverrides.register,
  });

  const workspaceId = $derived(data.workspace?.workspace_id ?? page.params.workspaceId ?? '');
  const ownsCurrentRoute = $derived(
    workspaceId !== '' && ownsRoutePath(workspaceRoute(workspaceId), page.url.pathname),
  );

  let activeWorkspaceId: string | null = null;

  function disposeWorkspaceResources(workspaceId: string): void {
    disposeWorkspaceWorkersStore(workspaceId);
    disposeWorkspaceMultiplexer(workspaceId);
  }

  $effect(() => {
    const nextWorkspaceId = data.workspace?.workspace_id ?? null;
    if (nextWorkspaceId === activeWorkspaceId) return;
    const previousWorkspaceId = activeWorkspaceId;
    activeWorkspaceId = nextWorkspaceId;
    if (previousWorkspaceId) disposeWorkspaceResources(previousWorkspaceId);
  });

  onDestroy(() => {
    if (activeWorkspaceId) disposeWorkspaceResources(activeWorkspaceId);
  });
</script>

{#snippet workspaceHeader()}
  <WorkspaceBreadcrumbs
    workspaceId={page.params.workspaceId ?? data.workspace?.workspace_id ?? ''}
    workspace={data.workspace ?? null}
    workspaceError={data.workspaceError ?? null}
  />
{/snippet}

{#snippet workspaceSidebar()}
  <WorkspaceSidebar
    workspace={data.workspace ?? null}
    workspaceError={data.workspaceError ?? null}
    currentPath={page.url.pathname}
    content={sidebarContent}
  />
{/snippet}

<HeaderOverride content={workspaceHeader} active={ownsCurrentRoute} />
<SidebarOverride
  controller={parentSidebarController}
  sidebar={workspaceSidebar}
  active={ownsCurrentRoute}
/>

{@render children()}
