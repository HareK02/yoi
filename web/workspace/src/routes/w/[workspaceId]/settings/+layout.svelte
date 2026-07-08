<script lang="ts">
  import { page } from '$app/state';
  import { workspaceRoute } from '$lib/workspace-api/http';
  import { SETTINGS_SECTIONS, settingsSectionHref } from '$lib/workspace-settings/model';
  import type { LayoutProps } from './$types';

  let { data, children }: LayoutProps = $props();

  let workspaceId = $derived(data.workspace?.workspace_id ?? '');
  let settingsHref = $derived(workspaceId ? workspaceRoute(workspaceId, '/settings') : '/settings');

  function sectionHref(path: string): string {
    return workspaceId ? workspaceRoute(workspaceId, path) : path;
  }

  function isActive(path: string): boolean {
    const href = sectionHref(path);
    return page.url.pathname === href || page.url.pathname.startsWith(`${href}/`);
  }
</script>

<section class="settings-page">
  <div class="settings-hero">
    <p class="eyebrow">Settings / Admin</p>
    <h1>Workspace settings</h1>
    <p>
      Configure workspace metadata, runtime connections, and Decodal profile sources through the Backend.
    </p>
  </div>

  <nav class="settings-nav" aria-label="Settings sections">
    <a class:active={page.url.pathname === settingsHref} href={settingsHref}>
      <span>Overview</span>
      <small>Settings map</small>
    </a>
    {#each SETTINGS_SECTIONS as section}
      {@const href = sectionHref(settingsSectionHref(section.id))}
      <a class:active={isActive(settingsSectionHref(section.id))} href={href}>
        <span>{section.label}</span>
        <small>{section.status}</small>
      </a>
    {/each}
  </nav>

  {@render children()}
</section>
