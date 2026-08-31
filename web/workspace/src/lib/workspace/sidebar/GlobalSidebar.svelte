<script lang="ts">
  import type { SidebarSnippet } from './context';
  import './sidebar.css';

  type Props = {
    currentPath: string;
    content?: SidebarSnippet | null;
  };

  const { currentPath, content = null }: Props = $props();

  const items = [
    { href: '/', label: 'Workspaces' },
    { href: '/#workspace-create-title', label: 'Create Workspace' },
    { href: '/account', label: 'Account' },
    { href: '/login/device', label: 'Device Login' },
  ];
</script>

{#if content}
  {@render content()}
{:else}
  <div class="global-sidebar" aria-label="Global navigation">
    <div class="global-sidebar-section">
      <nav class="sidebar-list" aria-label="Global pages">
        {#each items as item}
          <a
            class="sidebar-link"
            class:active={currentPath === item.href}
            href={item.href}
            aria-current={currentPath === item.href ? 'page' : undefined}
          >
            <span>{item.label}</span>
          </a>
        {/each}
      </nav>
    </div>
  </div>
{/if}
