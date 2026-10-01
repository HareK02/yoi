<script lang="ts">
  import WorkspaceIdentitySettings from '#lib/workspace/settings/WorkspaceIdentitySettings.svelte';
  import type { PageProps } from './$types';
  let { data }: PageProps = $props();
</script>

<svelte:head><title>Workspace Identity · Settings · Yoi</title></svelte:head>

{#if data.workspace}
  {#key `${data.workspace.workspace_id}:${data.workspace.permissions.delete_workspace}`}
    <WorkspaceIdentitySettings
      workspaceId={data.workspace.workspace_id}
      workspaceName={data.workspace.display_name}
      canDelete={data.workspace.permissions.delete_workspace}
    />
  {/key}
{/if}
