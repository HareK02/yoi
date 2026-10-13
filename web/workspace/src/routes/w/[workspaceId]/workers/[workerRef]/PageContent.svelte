<script lang="ts">
  import { workerConsoleHref } from '#lib/workspace/resource-links.ts';
  import { liveWorkerState } from '#lib/workspace/sidebar/worker-state.ts';
  import type { ResolvedPageData } from '#lib/workspace/navigation/deferred.ts';
  import type { PageData as DeferredPageData } from './$types';
  type PageData = ResolvedPageData<DeferredPageData>;
  let { data }: { data: PageData } = $props();
</script>

<svelte:head><title>{data.worker?.resource_key ?? 'Worker'} · Yoi</title></svelte:head>

<section class="workspace-page-shell">
  {#if data.workerError}
    <p class="error-message">{data.workerError}</p>
  {:else if data.worker}
    <header class="workspace-page-header">
      <div>
        <p class="eyebrow">{data.worker.resource_key}</p>
        <h1>{data.worker.display_name}</h1>
      </div>
      <a
        class="button-primary"
        href={workerConsoleHref(data.workspaceId, data.worker)}
      >Open console</a>
    </header>
    <dl class="resource-meta">
      <dt>Status</dt><dd>{liveWorkerState(data.worker)}</dd>
      <dt>Profile</dt><dd>{data.worker.profile}</dd>
      <dt>Internal ID</dt><dd><code>{data.worker.worker_id}</code></dd>
    </dl>
  {/if}
</section>
