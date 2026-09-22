<script lang="ts">
  import { page } from '$app/state';
  import { setContext } from 'svelte';
  import SettingsSidebar from '$lib/workspace/sidebar/SettingsSidebar.svelte';
  import SidebarOverride from '$lib/workspace/sidebar/SidebarOverride.svelte';
  import {
    getSidebarController,
    SIDEBAR_CONTEXT,
    type SidebarController,
    type SidebarSnippet,
  } from '$lib/workspace/sidebar/context';
  import { createOverrideStack } from '$lib/workspace/sidebar/override-stack';
  import { ownsRoutePath } from '$lib/workspace/sidebar/route-ownership';
  import { workspaceRoute } from '$lib/workspace/api/http';
  import '$lib/workspace/styles/settings.css';
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
    workspaceId !== '' &&
      ownsRoutePath(workspaceRoute(workspaceId, '/settings'), page.url.pathname),
  );
</script>

{#snippet settingsSidebar()}
  <SettingsSidebar
    workspaceId={page.params.workspaceId ?? ''}
    currentPath={page.url.pathname}
    content={sidebarContent}
  />
{/snippet}

<SidebarOverride
  controller={parentSidebarController}
  sidebar={settingsSidebar}
  active={ownsCurrentRoute}
/>

<section class="settings-page">
  {@render children()}
</section>
