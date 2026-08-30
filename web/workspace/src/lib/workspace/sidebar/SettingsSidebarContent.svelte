<script lang="ts">
  import { workspaceRoute } from '$lib/workspace/api/http';
  import { SETTINGS_SECTIONS, SETTINGS_ROUTE, settingsSectionHref } from '$lib/workspace/settings/model';

  let { workspaceId, currentPath }: { workspaceId: string; currentPath: string } = $props();

  let settingsHref = $derived(workspaceId ? workspaceRoute(workspaceId, SETTINGS_ROUTE) : SETTINGS_ROUTE);

  function sectionHref(path: string): string {
    return workspaceId ? workspaceRoute(workspaceId, path) : path;
  }

  function isActive(href: string): boolean {
    return currentPath === href || currentPath.startsWith(`${href}/`);
  }
</script>

<nav class="sidebar-sections" aria-label="Settings sections">
  <div class="nav-section">
    <div class="section-heading">
      <h2>Settings</h2>
    </div>
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
