<script lang="ts">
  import { page } from '$app/state';
  import WorkspaceSidebar from '$lib/workspace-sidebar/WorkspaceSidebar.svelte';
  import '../app.css';
  import type { LayoutProps } from './$types';

  let { data, children }: LayoutProps = $props();
</script>

<div class="workspace-layout">
  <WorkspaceSidebar
    workspace={data.workspace}
    workspaceError={data.workspaceError}
    repositories={data.repositories}
    repositoriesError={data.repositoriesError}
    currentPath={page.url.pathname}
  />
  <main class="shell">
    {@render children()}
  </main>
</div>
