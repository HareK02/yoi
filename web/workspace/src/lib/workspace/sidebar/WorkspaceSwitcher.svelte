<script lang="ts">
  import { goto } from "$app/navigation";
  import { onMount } from "svelte";
  import {
    listWorkspaces,
    type WorkspaceCatalogRecord,
  } from "$lib/workspace/api/workspace-catalog";
  import "$lib/workspace/styles/workspace-catalog.css";

  let { currentWorkspaceId } = $props<{ currentWorkspaceId: string }>();
  let workspaces = $state<WorkspaceCatalogRecord[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);

  onMount(async () => {
    try {
      workspaces = await listWorkspaces(fetch);
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      loading = false;
    }
  });

  async function switchWorkspace(event: Event) {
    const workspaceId = (event.currentTarget as HTMLSelectElement).value;
    if (!workspaceId || workspaceId === currentWorkspaceId) return;
    await goto(`/w/${encodeURIComponent(workspaceId)}`);
  }
</script>

<div class="workspace-switcher">
  <label for="workspace-switcher-select">Workspace</label>
  <select
    id="workspace-switcher-select"
    value={currentWorkspaceId}
    onchange={switchWorkspace}
    disabled={loading}
    aria-label="Switch Workspace"
  >
    {#if !workspaces.some((workspace) => workspace.workspace_id === currentWorkspaceId)}
      <option value={currentWorkspaceId}>
        {loading ? "Loading current Workspace…" : "Current Workspace unavailable"}
      </option>
    {/if}
    {#each workspaces as workspace (workspace.workspace_id)}
      <option value={workspace.workspace_id}>{workspace.display_name}</option>
    {/each}
  </select>
  <div class="workspace-switcher-actions">
    <a href="/">All Workspaces</a>
    <a href="/#workspace-create-title">Create</a>
  </div>
  {#if error}<span class="workspace-switcher-error">Selector unavailable: {error}</span>{/if}
</div>
