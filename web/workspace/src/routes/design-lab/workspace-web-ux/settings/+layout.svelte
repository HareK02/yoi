<script lang="ts">
  import { page } from '$app/state';
  import { setContext } from 'svelte';
  import SidebarOverride from '$lib/workspace/sidebar/SidebarOverride.svelte';
  import {
    getSidebarController,
    SIDEBAR_CONTEXT,
    type SidebarController,
    type SidebarSnippet,
  } from '$lib/workspace/sidebar/context';
  import { createOverrideStack } from '$lib/workspace/sidebar/override-stack';
  import { ownsRoutePath } from '$lib/workspace/sidebar/route-ownership';
  import SettingsSidebarFixture from './SettingsSidebarFixture.svelte';
  import { designLabBasePath } from '../workspace-navigation';
  import type { LayoutProps } from './$types';

  let { children }: LayoutProps = $props();
  const parentSidebarController = getSidebarController();
  let sidebarContent = $state<SidebarSnippet | null>(null);
  const sidebarContentOverrides = createOverrideStack<SidebarSnippet>((activeContent) => {
    sidebarContent = activeContent;
  });

  setContext<SidebarController>(SIDEBAR_CONTEXT, {
    registerSidebar: sidebarContentOverrides.register,
  });

  const ownsCurrentRoute = $derived(
    ownsRoutePath(`${designLabBasePath}/settings`, page.url.pathname),
  );
</script>

{#snippet settingsSidebar()}
  <SettingsSidebarFixture currentPath={page.url.pathname} content={sidebarContent} />
{/snippet}

<SidebarOverride
  controller={parentSidebarController}
  sidebar={settingsSidebar}
  active={ownsCurrentRoute}
/>

{@render children()}
