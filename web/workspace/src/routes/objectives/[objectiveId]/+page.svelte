<script lang="ts">
  import WorkspacePage from '$lib/workspace-pages/WorkspacePage.svelte';

  let { data }: { data: { objectiveId: string } } = $props();
</script>

<WorkspacePage view="objective" objectiveId={data.objectiveId} />
