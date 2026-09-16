<script lang="ts">
  import type { Snippet } from 'svelte';
  import { settingsBasePath, settingsNavigation } from './settings-navigation';

  let {
    currentPath,
    content = null,
  }: {
    currentPath: string;
    content?: Snippet<[]> | null;
  } = $props();
</script>

<div class="settings-sidebar">
  {#if content}
    {@render content()}
  {:else}
    <nav class="sidebar-sections" aria-label="Settings sections">
      <div class="sidebar-nav-section">
        <div class="sidebar-list">
          {#each settingsNavigation as item, index}
            <a
              class="sidebar-link"
              class:active={index === 0 && currentPath === settingsBasePath}
              href={item.href}
              aria-current={index === 0 && currentPath === settingsBasePath ? 'page' : undefined}
            >
              <span class="sidebar-link-label">{item.label}</span>
            </a>
          {/each}
        </div>
      </div>
    </nav>
  {/if}
</div>
