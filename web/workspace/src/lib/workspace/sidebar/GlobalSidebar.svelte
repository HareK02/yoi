<script lang="ts">
  import type { SidebarSnippet } from './context';
  import GlobalNavSections from './GlobalNavSections.svelte';
  import './sidebar.css';

  type Props = {
    currentPath: string;
    content?: SidebarSnippet | null;
  };

  const { currentPath, content = null }: Props = $props();
</script>

{#if content}
  {@render content()}
{:else}
  <div class="global-sidebar" aria-label="Global navigation">
    <div class="global-sidebar-section">
      <GlobalNavSections {currentPath} />
    </div>
  </div>
{/if}
