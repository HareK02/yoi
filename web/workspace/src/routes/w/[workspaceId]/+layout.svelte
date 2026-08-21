<script lang="ts">
  import { page } from '$app/state';
  import HeaderOverride from '$lib/workspace/header/HeaderOverride.svelte';
  import WorkspaceBreadcrumbs from '$lib/workspace/header/WorkspaceBreadcrumbs.svelte';
  import SidebarOverride from '$lib/workspace/sidebar/SidebarOverride.svelte';
  import { disposeWorkspaceMultiplexer } from '$lib/workspace/multiplexer';
  import WorkspaceSidebar from '$lib/workspace/sidebar/WorkspaceSidebar.svelte';
  import '$lib/workspace/styles/workspace-pages.css';
  import '$lib/workspace/styles/tickets.css';
  import '$lib/workspace/styles/workers.css';
  import type { LayoutProps } from './$types';

  let { data, children }: LayoutProps = $props();
  $effect(() => {
    const workspaceId = data.workspace?.workspace_id;
    if (!workspaceId) return;
    return () => disposeWorkspaceMultiplexer(workspaceId);
  });
</script>

{#snippet workspaceHeader()}
  <WorkspaceBreadcrumbs workspaceId={page.params.workspaceId ?? data.workspace?.workspace_id ?? ''} />
{/snippet}

{#snippet workspaceSidebar()}
  <WorkspaceSidebar
    workspace={data.workspace ?? null}
    workspaceError={data.workspaceError ?? null}
    currentPath={page.url.pathname}
  />
{/snippet}

<HeaderOverride content={workspaceHeader} />
<SidebarOverride sidebar={workspaceSidebar} />

{@render children()}
