<script lang="ts">
  import type { Snippet } from 'svelte';
  import { ownsRoutePath } from '$lib/workspace/sidebar/route-ownership';
  import { designLabBasePath, workspaceNavigation, workspaceWorkers } from './workspace-navigation';

  let {
    currentPath,
    content = null,
  }: {
    currentPath: string;
    content?: Snippet<[]> | null;
  } = $props();

  const settingsPath = `${designLabBasePath}/settings`;
</script>

<div class="workspace-sidebar">
  <header class="sidebar-header">
    <nav class="workspace-sidebar-shortcuts" aria-label="Workspace shortcuts">
      <a
        class="workspace-sidebar-shortcut"
        class:active={currentPath === designLabBasePath}
        href={designLabBasePath}
        aria-label="Workspace home"
        title="Workspace home"
        aria-current={currentPath === designLabBasePath ? 'page' : undefined}
      >
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path d="m3 11 9-8 9 8"></path>
          <path d="M5 10v10h14V10"></path>
          <path d="M9 20v-6h6v6"></path>
        </svg>
      </a>
      <a
        class="workspace-sidebar-shortcut"
        class:active={ownsRoutePath(settingsPath, currentPath)}
        href={settingsPath}
        aria-label="Workspace settings"
        title="Workspace settings"
        aria-current={currentPath === settingsPath ? 'page' : undefined}
      >
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path d="M4 5h16M4 12h16M4 19h16"></path>
          <path d="M8 3v4M16 10v4M10 17v4"></path>
        </svg>
      </a>
    </nav>
  </header>

  {#if content}
    {@render content()}
  {:else}
    <nav class="sidebar-sections" aria-label="Workspace sections">
      {#each workspaceNavigation as item}
        {#if item.children}
          <section class="sidebar-nav-section sidebar-nav-section--category">
            <h2 class="sidebar-nav-section__header">{item.label}</h2>
            {#each item.children as child}
              <a class="sidebar-link" href={child.href}>{child.label}</a>
            {/each}
          </section>
        {:else if item.label === 'Workers'}
          <section class="sidebar-nav-section" aria-labelledby="design-lab-workers-heading">
            <div class="section-heading-row">
              <h2 id="design-lab-workers-heading">
                <a class="section-heading-link" href={item.href}>workers</a>
              </h2>
              <a class="section-action" href={`${designLabBasePath}?worker=new`}>New</a>
              <span class="section-count">{workspaceWorkers.length}</span>
            </div>
            <ul class="nav-list" aria-label="Workers">
              {#each workspaceWorkers as worker}
                <li class="worker-nav-item">
                  <a class="worker-nav-link" href={`${designLabBasePath}?worker=${worker.key}`}>
                    <span class="worker-status-indicator" aria-hidden="true">
                      <span class="worker-status-dot"></span>
                    </span>
                    <span class="worker-nav-label" title={worker.label}>{worker.label}</span>
                    <small class="worker-nav-meta" title={`${worker.state} · ${worker.repository}`}>
                      {worker.state} · {worker.repository}
                    </small>
                  </a>
                </li>
              {/each}
            </ul>
          </section>
        {:else}
          <section class="sidebar-nav-section sidebar-nav-section--resource">
            <a class="sidebar-link" href={item.href}>{item.label}</a>
          </section>
        {/if}
      {/each}
    </nav>
  {/if}
</div>
