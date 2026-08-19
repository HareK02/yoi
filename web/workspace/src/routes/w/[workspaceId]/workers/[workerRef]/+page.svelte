<script lang="ts">
  import { workspaceRoute } from '$lib/workspace/api/http';
  import type { PageData } from './$types';
  let { data }: { data: PageData } = $props();
</script>

<svelte:head><title>{data.worker?.human_key ?? 'Worker'} · Yoi</title></svelte:head>

<section class="workspace-page-shell">
  {#if data.workerError}
    <p class="error-message">{data.workerError}</p>
  {:else if data.worker}
    <header class="workspace-page-header">
      <div>
        <p class="eyebrow">{data.worker.human_key}</p>
        <h1>{data.worker.display_name}</h1>
      </div>
      <a
        class="button-primary"
        href={workspaceRoute(
          data.workspaceId,
          `/runtimes/${data.worker.runtime_id}/workers/${data.worker.worker_id}/console`,
        )}
      >Open console</a>
    </header>
    <dl class="resource-meta">
      <dt>Status</dt><dd>{data.worker.state}</dd>
      <dt>Profile</dt><dd>{data.worker.profile}</dd>
      <dt>Internal ID</dt><dd><code>{data.worker.worker_id}</code></dd>
    </dl>
  {/if}
</section>
