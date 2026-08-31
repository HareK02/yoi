<script lang="ts">
  import { setContext } from 'svelte';
  import { page } from '$app/state';
  import HeaderOverride from '$lib/workspace/header/HeaderOverride.svelte';
  import WorkspaceBreadcrumbs from '$lib/workspace/header/WorkspaceBreadcrumbs.svelte';
  import SidebarOverride from '$lib/workspace/sidebar/SidebarOverride.svelte';
  import {
    getSidebarController,
    SIDEBAR_CONTEXT,
    type SidebarController,
    type SidebarSnippet,
  } from '$lib/workspace/sidebar/context';
  import { createOverrideStack } from '$lib/workspace/sidebar/override-stack';
  import { disposeWorkspaceMultiplexer } from '$lib/workspace/multiplexer';
  import WorkspaceSidebar from '$lib/workspace/sidebar/WorkspaceSidebar.svelte';
  import '$lib/workspace/styles/workspace-pages.css';
  import '$lib/workspace/styles/tickets.css';
  import '$lib/workspace/styles/workers.css';
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

  $effect(() => {
    const workspaceId = data.workspace?.workspace_id;
    if (!workspaceId) return;
    return () => disposeWorkspaceMultiplexer(workspaceId);
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

<HeaderOverride content={workspaceHeader} />
<SidebarOverride controller={parentSidebarController} sidebar={workspaceSidebar} />

{@render children()}
