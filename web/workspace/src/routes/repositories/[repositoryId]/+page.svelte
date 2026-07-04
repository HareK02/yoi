<script lang="ts">
  import { page } from '$app/state';
  import WorkspacePage from '$lib/workspace-pages/WorkspacePage.svelte';
</script>

<WorkspacePage view="repository" repositoryId={page.params.repositoryId} />
