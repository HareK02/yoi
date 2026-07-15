<script lang="ts">
  import { workspaceRoute } from '$lib/workspace/api/http';
  import { SETTINGS_SECTIONS, settingsSectionHref } from '$lib/workspace/settings/model';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
  let workspaceId = $derived(data.workspace?.workspace_id ?? '');

  function href(path: string): string {
    return workspaceId ? workspaceRoute(workspaceId, path) : path;
  }
</script>

<section class="settings-card">
  <div class="settings-card-header">
    <div>
      <p class="eyebrow">Overview</p>
      <h2>Settings areas</h2>
    </div>
  </div>
  <div class="settings-section-grid">
    {#each SETTINGS_SECTIONS as section}
      <a class="settings-section-card" href={href(settingsSectionHref(section.id))}>
        <span class="section-status-pill">{section.status}</span>
        <h3>{section.label}</h3>
        <p>{section.summary}</p>
      </a>
    {/each}
  </div>
</section>

<section class="settings-card">
  <div class="settings-card-header">
    <div>
      <p class="eyebrow">Backend pattern</p>
      <h2>Authority boundaries</h2>
    </div>
  </div>
  <div class="settings-pattern-grid">
    <article>
      <h3>Backend-owned state</h3>
      <p>
        Workspace metadata and Decodal profile sources are edited through Backend APIs, not direct frontend filesystem access.
      </p>
    </article>
    <article>
      <h3>Runtime inputs</h3>
      <p>
        Runtime execution receives explicit resources and WorkingDirectory handles; raw workspace paths stay out of Browser-facing payloads.
      </p>
    </article>
    <article>
      <h3>Diagnostics first</h3>
      <p>
        Settings mutations return typed diagnostics so validation issues are visible without exposing internal paths or credentials.
      </p>
    </article>
  </div>
</section>
