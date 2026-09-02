<script lang="ts">
  import type { WorkspaceCatalogRecord } from '../api/workspace-catalog';
  import type { SidebarSnippet } from './context';
  import GlobalNavSections from './GlobalNavSections.svelte';
  import './sidebar.css';

  type Props = {
    currentPath: string;
    content?: SidebarSnippet | null;
    workspaces?: WorkspaceCatalogRecord[];
    workspaceError?: string | null;
  };

  const {
    currentPath,
    content = null,
    workspaces = [],
    workspaceError = null,
  }: Props = $props();
</script>

{#if content}
  {@render content()}
{:else}
  <div class="global-sidebar" aria-label="Global navigation">
    <div class="global-sidebar-section">
      <GlobalNavSections {currentPath} {workspaces} {workspaceError} />
    </div>
  </div>
{/if}
