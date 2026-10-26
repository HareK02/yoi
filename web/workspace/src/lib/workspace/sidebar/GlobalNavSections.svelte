<script lang="ts">
  import type { WorkspaceCatalogRecord } from "#lib/workspace/api/workspace-catalog.ts";
  import { onMount, untrack } from "svelte";
  import { browserSession } from '#lib/workspace/auth/session-state.ts';
  import { loadHostingAccess, SESSION_CHANGED_EVENT } from "#lib/workspace/auth/hosting.ts";

  let mounted = $state(false);
  let isAdmin = $state(false);
  let accessRequest = 0;

  async function refreshAccess() {
    const request = ++accessRequest;
    isAdmin = false;
    try {
      // ページが確認したsessionを共有し、Sidebar追加でwhoamiの再取得を増やさない。
      const session = browserSession.snapshot;
      if (session.kind !== 'ready' || !session.value.actor) return;
      const access = await loadHostingAccess();
      if (request === accessRequest) isAdmin = access.is_admin;
    } catch {
      // 未確認の権限では管理導線を表示しない。直接screenでも独立して検証する。
    }
  }

  onMount(() => {
    mounted = true;
    const refresh = () => { void refreshAccess(); };
    const unsubscribe = browserSession.subscribe(refresh);
    window.addEventListener(SESSION_CHANGED_EVENT, refresh);
    window.addEventListener("focus", refresh);
    return () => {
      ++accessRequest;
      unsubscribe();
      window.removeEventListener(SESSION_CHANGED_EVENT, refresh);
      window.removeEventListener("focus", refresh);
    };
  });

  $effect(() => {
    currentPath;
    if (mounted) untrack(() => { void refreshAccess(); });
  });

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
    { href: "/dashboard", label: "Dashboard / Workspaces" },
    { href: "/dashboard#workspace-create-title", label: "Create Workspace" },
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
      {#if isAdmin}
        <a
          class="sidebar-link"
          class:active={currentPath === "/hosting/settings"}
          href="/hosting/settings"
          aria-current={currentPath === "/hosting/settings" ? "page" : undefined}
        >Hosting settings</a>
      {/if}
    </div>
  </section>

  {#if workspaces !== null}
    <section class="sidebar-nav-section sidebar-nav-section--category" aria-labelledby="global-workspaces-heading">
      <h2 id="global-workspaces-heading" class="sidebar-nav-section__header">workspaces</h2>

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
