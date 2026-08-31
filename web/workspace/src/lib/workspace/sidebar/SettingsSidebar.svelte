<script lang="ts">
  import { workspaceRoute } from '$lib/workspace/api/http';
  import { SETTINGS_SECTIONS, SETTINGS_ROUTE, settingsSectionHref } from '$lib/workspace/settings/model';
  import type { SidebarSnippet } from './context';

  let {
    workspaceId,
    currentPath,
    content = null,
  }: {
    workspaceId: string;
    currentPath: string;
    content?: SidebarSnippet | null;
  } = $props();

  let settingsHref = $derived(workspaceId ? workspaceRoute(workspaceId, SETTINGS_ROUTE) : SETTINGS_ROUTE);

  function sectionHref(path: string): string {
    return workspaceId ? workspaceRoute(workspaceId, path) : path;
  }

  function isActive(href: string): boolean {
    return currentPath === href || currentPath.startsWith(`${href}/`);
  }
</script>

<div class="settings-sidebar">
  <div class="section-heading">
    <h2>Settings</h2>
  </div>

  {#if content}
    {@render content()}
  {:else}
    <nav class="sidebar-sections" aria-label="Settings sections">
      <div class="sidebar-nav-section">
        <div class="sidebar-list">
          <a
            class:active={currentPath === settingsHref}
            class="sidebar-link"
            href={settingsHref}
            aria-current={currentPath === settingsHref ? 'page' : undefined}
          >
            <span class="sidebar-link-label">Overview</span>
          </a>
          {#each SETTINGS_SECTIONS as section}
            {@const href = sectionHref(settingsSectionHref(section.id))}
            <a
              class:active={isActive(href)}
              class="sidebar-link"
              href={href}
              aria-current={isActive(href) ? 'page' : undefined}
            >
              <span class="sidebar-link-label">{section.label}</span>
            </a>
          {/each}
        </div>
      </div>
    </nav>
  {/if}
</div>
