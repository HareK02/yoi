<script lang="ts">
  // root slotへWorkspace内容を登録し、子layoutには別のSidebar controllerを提供する。
  // Settings等の子はこのslotだけを差し替えるため、Workspace shortcut headerが残る。
  // Workspace切替と破棄時に旧subscriptionを解放し、別Workspaceのprojectionを混ぜない。
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

  import { observeDeferred } from '#lib/workspace/navigation/observe-deferred.svelte.ts';
  import { invalidateAll } from '$app/navigation';

  let { data, children, params }: LayoutProps = $props();
  const loaded = observeDeferred(() => data.deferredWorkspace, () => params.workspaceId);
  const workspace = $derived(loaded.data?.workspace ?? null);
  const workspaceError = $derived(loaded.failure?.message ?? null);
  const parentSidebarController = getSidebarController();
  let sidebarContent = $state<SidebarSnippet | null>(null);
  const sidebarContentOverrides = createOverrideStack<SidebarSnippet>((activeContent) => {
    sidebarContent = activeContent;
  });

  setContext<SidebarController>(SIDEBAR_CONTEXT, {
    registerSidebar: sidebarContentOverrides.register,
  });

  const workspaceId = $derived(params.workspaceId);
  const ownsCurrentRoute = $derived(
    workspaceId !== '' && ownsRoutePath(workspaceRoute(workspaceId), page.url.pathname),
  );

  let activeWorkspaceId: string | null = null;

  function disposeWorkspaceResources(workspaceId: string): void {
    disposeWorkspaceWorkersStore(workspaceId);
    disposeWorkspaceMultiplexer(workspaceId);
  }

  $effect(() => {
    const nextWorkspaceId = workspaceId || null;
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
    workspaceId={workspaceId}
    {workspace}
    {workspaceError}
  />
{/snippet}

{#snippet workspaceSidebar()}
  <WorkspaceSidebar
    {workspace}
    {workspaceError}
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

<svelte:head>
  {#if workspaceError}<title>Workspace · Load error</title>
  {:else if !workspace}<title>Workspace · Loading</title>{/if}
</svelte:head>

{#if workspaceError}
  <section class="workspace-page">
    <h1>Workspace details could not be loaded</h1>
    <p role="alert">{workspaceError}</p>
    <button type="button" class="workspace-secondary-button" onclick={() => invalidateAll()}>Retry</button>
  </section>
{:else if workspace}
  {@render children()}
{:else}
  <section class="workspace-page" aria-busy="true">
    <h1>Workspace</h1>
    <p role="status">Loading workspace…</p>
  </section>
{/if}
