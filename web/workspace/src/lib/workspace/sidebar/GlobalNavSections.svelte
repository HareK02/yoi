<script lang="ts">
  import type { WorkspaceCatalogRecord } from "$lib/workspace/api/workspace-catalog";

  type Props = {
    currentPath: string;
    workspaces?: WorkspaceCatalogRecord[] | null;
    workspaceError?: string | null;
  };

  let {
    currentPath,
    workspaces = null,
    workspaceError = null,
  }: Props = $props();

  const globalItems = [
    { href: "/#workspace-create-title", label: "Create Workspace" },
    { href: "/account", label: "Account" },
    { href: "/login/device", label: "Device Login" },
  ];

  function workspaceHref(workspaceId: string): string {
    return `/w/${encodeURIComponent(workspaceId)}`;
  }
</script>

<nav class="sidebar-sections" aria-label="Global pages">
  <section class="sidebar-nav-section">
    <div class="sidebar-list">
      {#each globalItems as item}
        <a
          class="sidebar-link"
          class:active={currentPath === item.href}
          href={item.href}
          aria-current={currentPath === item.href ? "page" : undefined}
        >
          <span>{item.label}</span>
        </a>
      {/each}
    </div>
  </section>

  {#if workspaces !== null}
    <section class="sidebar-nav-section sidebar-nav-section--category" aria-labelledby="global-workspaces-heading">
      <h2 id="global-workspaces-heading" class="sidebar-nav-section__header">workspaces</h2>

      {#if workspaceError}
        <p class="workspace-status error">Workspace list unavailable.</p>
      {/if}

      {#if workspaces.length > 0}
        <div class="sidebar-list">
          {#each workspaces as workspace (workspace.workspace_id)}
            {@const href = workspaceHref(workspace.workspace_id)}
            <a
              class="sidebar-link"
              class:active={currentPath === href}
              {href}
              aria-current={currentPath === href ? "page" : undefined}
            >
              <span>{workspace.display_name}</span>
            </a>
          {/each}
        </div>
      {:else if !workspaceError}
        <p class="workspace-status">No accessible Workspaces.</p>
      {/if}
    </section>
  {/if}
</nav>
