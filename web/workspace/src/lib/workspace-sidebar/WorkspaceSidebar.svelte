<script lang="ts">
  import { onMount } from 'svelte';
  import ObjectivesNavSection from './ObjectivesNavSection.svelte';
  import RepositoriesNavSection from './RepositoriesNavSection.svelte';
  import WorkersNavSection from './WorkersNavSection.svelte';
  import type { RepositoryListResponse, WorkspaceResponse } from './types';

  type Props = {
    workspace: WorkspaceResponse | null;
    workspaceError?: string | null;
    repositories?: RepositoryListResponse | null;
    repositoriesError?: string | null;
    currentPath?: string;
  };

  let {
    workspace,
    workspaceError = null,
    repositories,
    repositoriesError,
    currentPath = '/'
  }: Props = $props();

  let fallbackRepositories = $state<RepositoryListResponse | null>(null);
  let fallbackRepositoriesError = $state<string | null>(null);
  let displayedRepositories = $derived(repositories === undefined ? fallbackRepositories : repositories);
  let displayedRepositoriesError = $derived(
    repositoriesError === undefined ? fallbackRepositoriesError : repositoriesError
  );

  onMount(() => {
    if (repositories !== undefined) {
      return;
    }
    const controller = new AbortController();
    void loadFallbackRepositories(controller.signal);
    return () => controller.abort();
  });

  async function loadFallbackRepositories(signal?: AbortSignal) {
    fallbackRepositoriesError = null;
    try {
      const response = await fetch('/api/repositories', { signal });
      if (!response.ok) {
        throw new Error(`repositories request failed (${response.status})`);
      }
      fallbackRepositories = (await response.json()) as RepositoryListResponse;
    } catch (error) {
      if (error instanceof DOMException && error.name === 'AbortError') {
        return;
      }
      fallbackRepositoriesError = error instanceof Error ? error.message : 'repositories request failed';
      fallbackRepositories = null;
    }
  }
</script>

<aside class="workspace-sidebar" aria-label="Workspace navigation">
  <header class="sidebar-header">
    <div class="workspace-label">
      {#if workspace}
        <p class="workspace-status">{workspace.workspace_id}</p>
        <h1>{workspace.display_name}</h1>
      {:else}
        <h1>Yoi workspace</h1>
        {#if workspaceError}
          <p class="workspace-status error">Workspace summary unavailable.</p>
        {:else}
          <p class="workspace-status">Loading workspace…</p>
        {/if}
      {/if}
    </div>

    <a
      class="settings-button"
      href="/settings"
      aria-label="Open Settings / Admin"
      title="Settings / Admin"
    >
      ⚙
    </a>
  </header>

  <nav class="sidebar-sections" aria-label="Workspace sections">
    <RepositoriesNavSection
      repositories={displayedRepositories}
      repositoriesError={displayedRepositoriesError}
      {currentPath}
    />
    <ObjectivesNavSection {currentPath} />
    <WorkersNavSection {currentPath} />

  </nav>
</aside>
