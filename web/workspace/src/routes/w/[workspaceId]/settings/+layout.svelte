<script lang="ts">
  import { page } from '$app/state';
  import SettingsSidebarContent from '$lib/workspace/sidebar/SettingsSidebarContent.svelte';
  import WorkspaceSidebarContentOverride from '$lib/workspace/sidebar/WorkspaceSidebarContentOverride.svelte';
  import '$lib/workspace/styles/settings.css';
  import type { LayoutProps } from './$types';

  let { children }: LayoutProps = $props();
</script>

{#snippet settingsSidebarContent()}
  <SettingsSidebarContent
    workspaceId={page.params.workspaceId ?? ''}
    currentPath={page.url.pathname}
  />
{/snippet}

<WorkspaceSidebarContentOverride content={settingsSidebarContent} />

<section class="settings-page">
  {@render children()}
</section>
