<script lang="ts">
  import { page } from '$app/state';
  import WorkspaceAlerts from '$lib/workspace/alerts/WorkspaceAlerts.svelte';
  import GlobalSidebar from '$lib/workspace/sidebar/GlobalSidebar.svelte';
  import '../app.css';
  import type { LayoutProps } from './$types';

  let { data, children }: LayoutProps = $props();
</script>

<WorkspaceAlerts />

<div class="app-layout">
  <header class="workspace-topbar">
    <nav class="workspace-topbar-actions" aria-label="Global navigation">
      <a class="topbar-icon-button" href="/account" aria-label="Open Account" title="Account">
        <svg class="topbar-icon" aria-hidden="true" viewBox="0 0 24 24">
          <path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2" />
          <circle cx="12" cy="7" r="4" />
        </svg>
      </a>
    </nav>
  </header>
  <div class:global-sidebar-layout={!data.workspaceScoped} class="app-shell">
    {#if !data.workspaceScoped}
      <GlobalSidebar currentPath={page.url.pathname} />
    {/if}
    <div class="app-content">
      {@render children()}
    </div>
  </div>
</div>
