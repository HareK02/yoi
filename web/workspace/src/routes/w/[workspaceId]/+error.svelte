<script lang="ts">
  import { page } from "$app/state";
  import "$lib/workspace/styles/workspace-catalog.css";

  const workspaceId = $derived(page.params.workspaceId ?? "unknown");
</script>

<svelte:head><title>Workspace unavailable · Yoi</title></svelte:head>

<div class="workspace-catalog-shell">
  <section class="workspace-empty-state">
    <p class="workspace-catalog-eyebrow">Workspace unavailable</p>
    <h1>The selected Workspace cannot be opened</h1>
    <p>
      <code>{workspaceId}</code> may have been removed, become inaccessible, or no longer exist on
      this Backend. No state from a previously selected Workspace was retained.
    </p>
    <div class="workspace-switcher-actions">
      <a href="/">Choose another Workspace</a>
      <a href="/#workspace-create-title">Create Workspace</a>
    </div>
  </section>
</div>
