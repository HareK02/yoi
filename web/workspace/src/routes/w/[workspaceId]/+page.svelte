<script lang="ts">
  import { workspaceRoute } from '$lib/workspace/api/http';
  import BevelLine from '$lib/workspace/ui/BevelLine.svelte';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
  const resources = [
    { label: 'Tickets', path: '/tickets' },
    { label: 'Objectives', path: '/objectives' },
    { label: 'Merge Requests', path: '/merge-requests' },
    { label: 'Memory', path: '/memory' },
    { label: 'Workers', path: '/workers' },
  ];
  const settings = $derived([
    { label: 'Runtimes', path: '/settings/runtimes', visible: data.workspace?.permissions.manage_runtimes === true },
    { label: 'Configuration Sources', path: '/settings/configuration', visible: true },
    { label: 'Repositories', path: '/settings/repositories', visible: data.workspace?.permissions.manage_repositories === true },
    { label: 'Repository Access', path: '/settings/repository-access', visible: data.workspace?.permissions.manage_secrets === true },
    { label: 'Profile Sources', path: '/settings/profiles', visible: true },
    { label: 'Workspace Identity', path: '/settings', visible: data.workspace?.permissions.delete_workspace === true },
  ].filter((item) => item.visible));
</script>

<svelte:head>
  <title>{data.workspace?.display_name ?? 'Workspace'} · Yoi</title>
</svelte:head>

<div class="workspace-home">
  {#if data.workspace}
    <div class="home-columns">
      <section aria-labelledby="home-work-heading">
        <h1 id="home-work-heading">Work</h1>
        <nav aria-labelledby="home-work-heading">
          <ul>
            {#each resources as resource, index (resource.path)}
              <li>
                <a class="resource-link" href={workspaceRoute(data.workspace.workspace_id, resource.path)}>
                  <span>{resource.label}</span>
                  <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m9 6 6 6-6 6" /></svg>
                </a>
                {#if index < resources.length - 1}<BevelLine decorative />{/if}
              </li>
            {/each}
          </ul>
        </nav>
      </section>
      <section class="home-settings" aria-labelledby="home-settings-heading">
        <h2 id="home-settings-heading">Settings</h2>
        <nav aria-labelledby="home-settings-heading">
          <ul>
            {#each settings as item (item.path)}
              <li><a href={workspaceRoute(data.workspace.workspace_id, item.path)}>{item.label}</a></li>
            {/each}
          </ul>
        </nav>
      </section>
    </div>
  {:else if data.workspaceError}
    <p class="home-error" role="alert">{data.workspaceError}</p>
    <a class="recovery-link" href="/">Choose a workspace</a>
  {:else}
    <p class="home-loading" role="status">Loading workspace…</p>
  {/if}
</div>

<style>
  .workspace-home {
    container-type: inline-size;
    min-width: 0;
  }
  .home-columns {
    display: grid;
    gap: var(--space-6);
    align-items: start;
  }
  section { min-width: 0; }
  h1 {
    margin: 0 0 var(--space-2);
    color: var(--text-strong);
    font-size: var(--font-size-title);
    line-height: var(--line-height-title);
  }
  h2 {
    margin: 0 0 var(--space-2);
    color: var(--text-muted);
    font-size: var(--font-size-body);
    line-height: var(--line-height-body);
  }
  ul { list-style: none; margin: 0; padding: 0; }
  a {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    min-height: 44px;
    padding-block: var(--space-3);
    color: var(--text);
    font-size: var(--font-size-body);
    line-height: var(--line-height-body);
    text-decoration: none;
    overflow-wrap: anywhere;
  }
  .resource-link { font-weight: 600; }
  a:hover, a:focus-visible { background: var(--interactive-hover); }
  a:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  a:active { background: var(--interactive-selected); }
  svg {
    flex: 0 0 auto;
    width: 16px;
    height: 16px;
    fill: none;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
    color: var(--text-muted);
  }
  .home-error, .home-loading { margin: 0; overflow-wrap: anywhere; }
  .home-error { color: var(--danger); }
  .home-loading { color: var(--text-muted); }
  .recovery-link { width: fit-content; text-decoration: underline; }
  @container (min-width: 48rem) {
    .home-columns { grid-template-columns: minmax(0, 2fr) minmax(0, 1fr); }
    .home-settings { padding-top: var(--space-3); }
  }
</style>
