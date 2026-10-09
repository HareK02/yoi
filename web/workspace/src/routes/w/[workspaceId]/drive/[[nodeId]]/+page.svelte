<script lang="ts">
  import { page } from '$app/state';
  import DrivePage from '#lib/workspace/drive-ui/DrivePage.svelte';
</script>
<svelte:head><title>Drive · Yoi Workspace</title><meta name="description" content="Workspace shared files" /></svelte:head>
<DrivePage workspaceId={page.params.workspaceId ?? ''} nodeId={page.params.nodeId} />
