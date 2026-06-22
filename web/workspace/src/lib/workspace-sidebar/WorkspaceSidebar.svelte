<script lang="ts">
  import ObjectivesNavSection from './ObjectivesNavSection.svelte';
  import RepositoriesNavSection from './RepositoriesNavSection.svelte';
  import WorkersNavSection from './WorkersNavSection.svelte';
  import type { WorkspaceResponse } from './types';

  type Props = {
    workspace: WorkspaceResponse | null;
    workspaceError?: string | null;
  };

  let { workspace, workspaceError = null }: Props = $props();
</script>

<aside class="workspace-sidebar" aria-label="Workspace navigation">
  <header class="sidebar-header">
    <div class="workspace-label">
      <span class="eyebrow">workspace</span>
      <h1>{workspace?.display_name ?? 'Yoi workspace'}</h1>
      {#if workspaceError}
        <p class="workspace-status error">Workspace summary unavailable.</p>
      {:else if workspace}
        <p class="workspace-status">{workspace.workspace_id}</p>
      {:else}
        <p class="workspace-status">Loading workspace…</p>
      {/if}
    </div>

    <button
      class="settings-button"
      type="button"
      aria-label="Workspace settings"
      title="Workspace settings placeholder"
      disabled
    >
      ⚙
    </button>
  </header>

  <nav class="sidebar-sections" aria-label="Workspace sections">
    <RepositoriesNavSection {workspace} />
    <ObjectivesNavSection />
    <WorkersNavSection />
  </nav>
</aside>

<style>
  .workspace-sidebar {
    align-self: stretch;
    min-width: 0;
    border: 1px solid rgba(148, 163, 184, 0.18);
    border-radius: 26px;
    background:
      linear-gradient(180deg, rgba(15, 23, 42, 0.96), rgba(15, 23, 42, 0.82)),
      rgba(15, 23, 42, 0.88);
    box-shadow: 0 24px 80px rgba(2, 6, 23, 0.28);
    padding: 18px;
  }

  .sidebar-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 14px;
    margin-bottom: 22px;
    min-width: 0;
  }

  .workspace-label {
    display: grid;
    gap: 5px;
    min-width: 0;
  }

  .eyebrow {
    color: #38bdf8;
    font-size: 0.72rem;
    font-weight: 800;
    letter-spacing: 0.16em;
    text-transform: uppercase;
  }

  h1,
  .workspace-status {
    overflow-wrap: anywhere;
  }

  h1 {
    margin: 0;
    color: #f8fafc;
    font-size: 1.1rem;
    line-height: 1.2;
  }

  .workspace-status {
    margin: 0;
    color: #94a3b8;
    font-size: 0.78rem;
    line-height: 1.35;
  }

  .workspace-status.error {
    color: #fecaca;
  }

  .settings-button {
    flex: 0 0 auto;
    display: grid;
    place-items: center;
    width: 34px;
    height: 34px;
    border: 1px solid rgba(148, 163, 184, 0.24);
    border-radius: 12px;
    background: rgba(15, 23, 42, 0.7);
    color: #cbd5e1;
    cursor: not-allowed;
  }

  .sidebar-sections {
    display: grid;
    gap: 24px;
    min-width: 0;
  }
</style>
