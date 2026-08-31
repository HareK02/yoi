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
  import '$lib/workspace/styles/settings.css';
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
</script>

{#snippet settingsSidebar()}
  <SettingsSidebar
    workspaceId={page.params.workspaceId ?? ''}
    currentPath={page.url.pathname}
    content={sidebarContent}
  />
{/snippet}

<SidebarOverride controller={parentSidebarController} sidebar={settingsSidebar} />

<section class="settings-page">
  {@render children()}
</section>
