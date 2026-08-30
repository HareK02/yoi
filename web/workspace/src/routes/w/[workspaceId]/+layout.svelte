<script lang="ts">
  import { setContext, type Snippet } from 'svelte';
  import { page } from '$app/state';
  import HeaderOverride from '$lib/workspace/header/HeaderOverride.svelte';
  import WorkspaceBreadcrumbs from '$lib/workspace/header/WorkspaceBreadcrumbs.svelte';
  import SidebarOverride from '$lib/workspace/sidebar/SidebarOverride.svelte';
  import { createOverrideStack } from '$lib/workspace/sidebar/override-stack';
  import {
    WORKSPACE_SIDEBAR_CONTENT_CONTEXT,
    type WorkspaceSidebarContentController,
  } from '$lib/workspace/sidebar/workspace-content-context';
  import { disposeWorkspaceMultiplexer } from '$lib/workspace/multiplexer';
  import WorkspaceSidebar from '$lib/workspace/sidebar/WorkspaceSidebar.svelte';
  import '$lib/workspace/styles/workspace-pages.css';
  import '$lib/workspace/styles/tickets.css';
  import '$lib/workspace/styles/workers.css';
  import type { LayoutProps } from './$types';

  let { data, children }: LayoutProps = $props();
  let sidebarContent = $state<Snippet | null>(null);
  const sidebarContentOverrides = createOverrideStack<Snippet>((activeContent) => {
    sidebarContent = activeContent;
  });

  setContext<WorkspaceSidebarContentController>(WORKSPACE_SIDEBAR_CONTENT_CONTEXT, {
    registerContent: sidebarContentOverrides.register,
  });

  $effect(() => {
    const workspaceId = data.workspace?.workspace_id;
    if (!workspaceId) return;
    return () => disposeWorkspaceMultiplexer(workspaceId);
  });
</script>

{#snippet workspaceHeader()}
  <WorkspaceBreadcrumbs workspaceId={page.params.workspaceId ?? data.workspace?.workspace_id ?? ''} />
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
<SidebarOverride sidebar={workspaceSidebar} />

{@render children()}
