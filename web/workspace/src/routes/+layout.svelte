<script lang="ts">
  import WorkspaceAlerts from '$lib/workspace/alerts/WorkspaceAlerts.svelte';
  import '../app.css';
  import type { LayoutProps } from './$types';

  let { children }: LayoutProps = $props();
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
  <div class="app-shell">
    {@render children()}
  </div>
</div>
