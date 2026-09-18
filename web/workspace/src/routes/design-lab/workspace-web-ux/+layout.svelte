<script lang="ts">
  import { page } from '$app/state';
  import { setContext } from 'svelte';
  import HeaderOverride from '$lib/workspace/header/HeaderOverride.svelte';
  import SidebarOverride from '$lib/workspace/sidebar/SidebarOverride.svelte';
  import {
    getSidebarController,
    SIDEBAR_CONTEXT,
    type SidebarController,
    type SidebarSnippet,
  } from '$lib/workspace/sidebar/context';
  import { createOverrideStack } from '$lib/workspace/sidebar/override-stack';
  import WorkspaceSidebarFixture from './WorkspaceSidebarFixture.svelte';
  import { designLabBasePath } from './workspace-navigation';
  import './showroom.css';
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

  const inSettings = $derived(page.url.pathname.startsWith(`${designLabBasePath}/settings`));
</script>

{#snippet designLabHeader()}
  <nav class="design-lab-header" aria-label="Current location">
    {#if inSettings}
      <a href={designLabBasePath}>Workspace Web UX</a>
      <span aria-hidden="true">/</span>
      <span aria-current="page">Settings</span>
    {:else}
      <span aria-current="page">Workspace Web UX</span>
    {/if}
  </nav>
{/snippet}

{#snippet workspaceSidebar()}
  <WorkspaceSidebarFixture currentPath={page.url.pathname} content={sidebarContent} />
{/snippet}

<HeaderOverride content={designLabHeader} />
<SidebarOverride controller={parentSidebarController} sidebar={workspaceSidebar} />

{@render children()}
