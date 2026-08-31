<script lang="ts">
  import { workspaceRoute } from '$lib/workspace/api/http';
  import { SETTINGS_SECTIONS, settingsSectionHref } from '$lib/workspace/settings/model';
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

  function sectionHref(path: string): string {
    return workspaceId ? workspaceRoute(workspaceId, path) : path;
  }

  function isActive(href: string): boolean {
    return currentPath === href || currentPath.startsWith(`${href}/`);
  }
</script>

<div class="settings-sidebar">
  {#if content}
    {@render content()}
  {:else}
    <nav class="sidebar-sections" aria-label="Settings sections">
      <div class="sidebar-nav-section">
        <div class="sidebar-list">
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
