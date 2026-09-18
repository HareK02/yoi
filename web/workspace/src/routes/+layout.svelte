<script lang="ts">
  import { page } from '$app/state';
  import { setContext } from 'svelte';
  import WorkspaceAlerts from '$lib/workspace/alerts/WorkspaceAlerts.svelte';
  import Bevel from '$lib/workspace/ui/Bevel.svelte';
  import { provideHeaderController, type HeaderController } from '$lib/workspace/header/context';
  import GlobalSidebar from '$lib/workspace/sidebar/GlobalSidebar.svelte';
  import SidebarFrame from '$lib/workspace/sidebar/SidebarFrame.svelte';
  import { SIDEBAR_CONTEXT, type SidebarController, type SidebarSnippet } from '$lib/workspace/sidebar/context';
  import { createOverrideStack } from '$lib/workspace/sidebar/override-stack';
  import '../app.css';
  import type { LayoutProps } from './$types';

  let { children, data }: LayoutProps = $props();
  let sidebar = $state<SidebarSnippet | null>(null);
  const sidebarOverrides = createOverrideStack<SidebarSnippet>((activeSidebar) => {
    sidebar = activeSidebar;
  });
  const headerController = $state<HeaderController>({ content: null });
  let sidebarFolded = $state(false);

  function toggleSidebar() {
    sidebarFolded = !sidebarFolded;
  }

  provideHeaderController(headerController);
  setContext<SidebarController>(SIDEBAR_CONTEXT, {
    registerSidebar: sidebarOverrides.register,
  });
</script>

<WorkspaceAlerts />

<div class="app-shell" class:sidebar-open={!sidebarFolded}>
  <SidebarFrame bind:folded={sidebarFolded}>
    <GlobalSidebar
      currentPath={page.url.pathname}
      content={sidebar}
      workspaces={data.accessibleWorkspaces}
      workspaceError={data.workspaceCatalogError}
    />
  </SidebarFrame>
  <Bevel as="div" class="app-shell__topbar-bevel" top={false} right={false} left={false} fill>
    <header class="app-shell__topbar">
      <div class="app-shell__topbar-location">
        {#if headerController.content}{@render headerController.content()}{/if}
      </div>
      <nav class="app-shell__topbar-actions" aria-label="Global navigation">
        <button
          class="app-shell__icon-button app-shell__mobile-sidebar-toggle"
          type="button"
          aria-label={sidebarFolded ? 'Show sidebar' : 'Hide sidebar'}
          aria-expanded={!sidebarFolded}
          title={sidebarFolded ? 'Show sidebar' : 'Hide sidebar'}
          onclick={toggleSidebar}
        >
          <svg class="app-shell__icon" aria-hidden="true" viewBox="0 0 24 24">
            {#if sidebarFolded}
              <path d="m6 17 5-5-5-5" />
              <path d="m13 17 5-5-5-5" />
            {:else}
              <path d="m11 17-5-5 5-5" />
              <path d="m18 17-5-5 5-5" />
            {/if}
          </svg>
        </button>
        <a class="app-shell__icon-button" href="/account" aria-label="Open Account" title="Account">
          <svg class="app-shell__icon" aria-hidden="true" viewBox="0 0 24 24">
            <path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2" />
            <circle cx="12" cy="7" r="4" />
          </svg>
        </a>
      </nav>
    </header>
  </Bevel>
  <main class="app-shell__main">
    {@render children()}
  </main>
</div>

<style>
  .app-shell {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    grid-template-rows: auto minmax(0, 1fr);
    width: 100vw;
    height: 100dvh;
    margin: 0;
    padding: 0;
    overflow: hidden;
    min-width: 0;
  }

  .app-shell :global(.bevel.app-shell__topbar-bevel) {
    z-index: 30;
    grid-column: 2;
    grid-row: 1;
    min-width: 0;
  }

  .app-shell__topbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-4);
    min-width: 0;
    min-height: 3.25rem;
    padding: 0 var(--space-5);
    background: color-mix(in srgb, var(--bg-raised) 88%, transparent);
    backdrop-filter: blur(14px);
  }

  .app-shell__topbar-location {
    flex: 1 1 auto;
    min-width: 0;
    overflow: visible;
  }

  .app-shell__topbar-actions {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
  }

  .app-shell__icon-button {
    display: inline-flex;
    width: 2.35rem;
    height: 2.35rem;
    align-items: center;
    justify-content: center;
    border-radius: 999px;
    color: var(--text-muted);
    text-decoration: none;
  }

  .app-shell__icon-button:hover,
  .app-shell__icon-button:focus-visible {
    background: var(--interactive-hover);
    color: var(--text-muted);
  }

  .app-shell__mobile-sidebar-toggle {
    display: none;
    padding: 0;
    border: 0;
    background: transparent;
    cursor: pointer;
  }

  .app-shell__icon {
    width: 1.1rem;
    height: 1.1rem;
    fill: none;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  .app-shell__main {
    grid-column: 2;
    grid-row: 2;
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
    min-width: 0;
    min-height: 0;
    width: 100%;
    max-width: 1280px;
    margin-inline: auto;
    overflow-y: auto;
    padding: var(--space-6);
  }

  @media (max-width: 760px) {
    .app-shell {
      grid-template-columns: minmax(0, 1fr);
      grid-template-rows: auto minmax(0, 1fr);
      width: 100vw;
      height: 100dvh;
      min-height: 0;
      overflow: hidden;
    }

    .app-shell :global(.bevel.app-shell__topbar-bevel) {
      grid-column: 1;
      grid-row: 1;
    }

    .app-shell__topbar {
      padding: 0 var(--space-4);
    }

    .app-shell__mobile-sidebar-toggle {
      display: inline-flex;
    }

    .app-shell__main {
      grid-column: 1;
      grid-row: 2;
      overflow-y: auto;
      padding: var(--space-5) var(--space-4);
    }

    .app-shell.sidebar-open .app-shell__main {
      display: none;
    }
  }
</style>
