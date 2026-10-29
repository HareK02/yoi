<script lang="ts">
  import SubjectPermissions from '#lib/workspace/memory/SubjectPermissions.svelte';
  import { workspaceRoute } from '#lib/workspace/api/http.ts';
  import type { ResolvedPageData } from '#lib/workspace/navigation/deferred.ts';
  import type { PageData } from './$types';
  let { data }: { data: ResolvedPageData<PageData> } = $props();
</script>
<svelte:head><title>Permissions · {data.subject.data?.role ?? 'Subject'} · Yoi Workspace</title></svelte:head>
<section class="permission-page">
  <a href={workspaceRoute(data.workspaceId, `/memory/${encodeURIComponent(data.subjectId)}`)}>← Subject details</a>
  <h1>{data.subject.data?.role ?? 'Subject'} permissions</h1>
  {#if data.subject.data}
    {#key JSON.stringify([data.workspaceId, data.subjectId])}<SubjectPermissions workspaceId={data.workspaceId} subjectId={data.subjectId} />{/key}
  {:else}<p role="alert">Target unavailable. {data.subject.error} An old page cannot recreate a deleted Subject.</p>{/if}
</section>
<style>.permission-page { padding: 1.5rem; min-width: 0; } h1 { font-size: var(--font-size-title); margin: 1rem 0 1.5rem; overflow-wrap: anywhere; } @media (max-width: 600px) { .permission-page { padding: 1rem; } }</style>
