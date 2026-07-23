<script lang="ts">
  import { page } from '$app/state';
  import SidebarOverride from '$lib/workspace/sidebar/SidebarOverride.svelte';
  import WorkspaceSidebar from '$lib/workspace/sidebar/WorkspaceSidebar.svelte';
  import type { LayoutProps } from './$types';

  let { data, children }: LayoutProps = $props();
</script>

{#snippet workspaceSidebar()}
  <WorkspaceSidebar
    workspace={data.workspace ?? null}
    workspaceError={data.workspaceError ?? null}
    repositories={data.repositories ?? null}
    repositoriesError={data.repositoriesError ?? null}
    currentPath={page.url.pathname}
  />
{/snippet}

<SidebarOverride sidebar={workspaceSidebar} />

{@render children()}
