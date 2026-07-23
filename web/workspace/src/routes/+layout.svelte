<script lang="ts">
  import { page } from '$app/state';
  import { setContext } from 'svelte';
  import WorkspaceAlerts from '$lib/workspace/alerts/WorkspaceAlerts.svelte';
  import GlobalSidebar from '$lib/workspace/sidebar/GlobalSidebar.svelte';
  import { SIDEBAR_CONTEXT, type SidebarSnippet } from '$lib/workspace/sidebar/context';
  import '../app.css';
  import type { LayoutProps } from './$types';

  let { children }: LayoutProps = $props();
  let sidebar = $state<SidebarSnippet | null>(null);

  setContext(SIDEBAR_CONTEXT, {
    setSidebar(snippet: SidebarSnippet) {
      sidebar = snippet;
    },
    clearSidebar(snippet: SidebarSnippet) {
      if (sidebar === snippet) sidebar = null;
    },
  });
</script>

<WorkspaceAlerts />

<div class="workspace-layout">
  {#if sidebar}
    {@render sidebar()}
  {:else}
    <GlobalSidebar currentPath={page.url.pathname} />
  {/if}
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
  <main class="shell">
    {@render children()}
  </main>
</div>
