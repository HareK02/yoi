<script lang="ts">
  import ConfigSourceEditor from "$lib/workspace/config-source/ConfigSourceEditor.svelte";
  import type { PageProps } from "./$types";

  let { data }: PageProps = $props();
  let workspaceId = $derived(data.workspace?.workspace_id ?? "");
  let workspaceName = $derived(data.workspace?.display_name ?? "Workspace");
</script>

<svelte:head>
  <title>Configuration | {workspaceName}</title>
</svelte:head>

<section class="settings-page">
  <header class="page-header">
    <div>
      <p class="eyebrow">Workspace settings</p>
      <h2>Configuration</h2>
      <p>
        Edit the Server-owned virtual Decodal source tree. Drafts stay in this browser;
        the Server persists only a fully evaluated candidate.
      </p>
    </div>
  </header>

  <ConfigSourceEditor {workspaceId} />
</section>
