<script lang="ts">
  import SubjectConversation from '#lib/workspace/memory/SubjectConversation.svelte';
  import { workspaceRoute } from '#lib/workspace/api/http.ts';
  import type { ResolvedPageData } from '#lib/workspace/navigation/deferred.ts';
  import type { PageData as RouteData } from './$types';
  let { data }: { data: ResolvedPageData<RouteData> } = $props();
  const subject = $derived(data.subject.data);
</script>

<svelte:head><title>{subject?.role ?? 'Subject'} · Conversation · Yoi Workspace</title></svelte:head>

<section class="conversation-page" aria-labelledby="conversation-subject-heading">
  <a href={workspaceRoute(data.workspaceId, `/memory/${encodeURIComponent(data.subjectId)}`)}>← Subject details</a>
  {#if subject}
    <h1 id="conversation-subject-heading">{subject.role}</h1>
    {#key JSON.stringify([data.accountId, data.workspaceId, subject.id])}
      <SubjectConversation accountId={data.accountId} workspaceId={data.workspaceId} subjectId={subject.id} subjectName={subject.role} />
    {/key}
  {:else}
    <h1 id="conversation-subject-heading">Subject conversation</h1>
    <p role="alert">{data.subject.error ?? 'The Subject is unavailable. No conversation input was sent.'}</p>
  {/if}
</section>

<style>
  .conversation-page { display: grid; gap: var(--space-4); padding: var(--space-4); width: 100%; box-sizing: border-box; max-width: 960px; min-width: 0; margin-inline: auto; }
  h1 { margin: 0; overflow-wrap: anywhere; }
</style>
