<script lang="ts">
  import { page } from '$app/state';
  import Members from '#lib/workspace/settings/Members.svelte';
</script>

<svelte:head><title>Members · Settings · Yoi Workspace</title></svelte:head>
<Members workspaceId={page.params.workspaceId ?? ''} />
