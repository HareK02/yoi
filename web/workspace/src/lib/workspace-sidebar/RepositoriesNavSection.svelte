<script lang="ts">
  import { projectRepositoryNav } from './repository-nav';
  import type { RepositoryListResponse } from './types';

  type Props = {
    repositories: RepositoryListResponse | null;
    repositoriesError?: string | null;
    currentPath?: string;
  };

  let { repositories, repositoriesError = null, currentPath = '/' }: Props = $props();
  let navigation = $derived(projectRepositoryNav(repositories, currentPath));
</script>

<section class="nav-section" aria-labelledby="repositories-heading">
  <div class="section-heading-row">
    <h2 id="repositories-heading">repositories</h2>
    <span class="section-count">{navigation.count}</span>
  </div>

  {#if repositoriesError}
    <p class="nav-empty error">Repository registry unavailable.</p>
  {:else if !repositories}
    <p class="nav-empty">Loading repositories…</p>
  {:else if navigation.items.length === 0}
    <p class="nav-empty">No repositories configured.</p>
    {#if navigation.diagnostics.length > 0}
      <ul class="diagnostics" aria-label="Repository diagnostics">
        {#each navigation.diagnostics as diagnostic}
          <li><code>{diagnostic.code}</code>: {diagnostic.message}</li>
        {/each}
      </ul>
    {/if}
  {:else}
    <ul class="nav-list" aria-label="Repositories">
      {#each navigation.items as item (item.id)}
        <li>
          <a class="nav-item" class:active={item.active} href={item.href} aria-current={item.active ? 'page' : undefined}>
            <span class="item-title">{item.title}</span>
            <span class="item-meta">{item.meta}</span>
          </a>
        </li>
      {/each}
    </ul>
  {/if}
</section>
