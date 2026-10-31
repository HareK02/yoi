<script lang="ts">
  import { subjectHref } from '#lib/workspace/subjektiv/routes.ts';
  import { untrack } from 'svelte';
  import { browserSession } from '#lib/workspace/auth/session-state.ts';
  import SubjectBody from '#lib/workspace/memory/SubjectBody.svelte';
  import SubjectConversation from '#lib/workspace/memory/SubjectConversation.svelte';
  import type { ResolvedPageData } from '#lib/workspace/navigation/deferred.ts';
  import type { PageData as RouteData } from './$types';
  let { data }: { data: ResolvedPageData<RouteData> } = $props();
  const subject = $derived(data.subject.data);
  // route再利用の描画はeffectより先に進むため、確認済みboolを別identityへ持ち越さない。
  let confirmedIdentity = $state<string | null>(null);
  const identity = $derived(JSON.stringify([data.accountId, data.workspaceId, data.subjectId]));
  const accountConfirmed = $derived(confirmedIdentity === identity);
  $effect(() => {
    const accountId = data.accountId;
    const workspaceId = data.workspaceId;
    const subjectId = data.subjectId;
    return untrack(() => {
      const generation = browserSession.authenticationGeneration;
      const stamp = browserSession.authenticationStamp;
      return browserSession.subscribe((snapshot) => {
        confirmedIdentity = data.workspaceId === workspaceId && data.subjectId === subjectId &&
          browserSession.authenticationGeneration === generation && browserSession.authenticationStamp === stamp &&
          snapshot.kind === 'ready' && snapshot.value.session.kind === 'authenticated' &&
          snapshot.value.actor?.account_id === accountId
          ? JSON.stringify([accountId, workspaceId, subjectId]) : null;
      });
    });
  });
</script>

<svelte:head><title>{subject?.role ?? 'Subject'} · Conversation · Yoi Workspace</title></svelte:head>

<section class="conversation-page" aria-labelledby="conversation-subject-heading">
  <a href={subjectHref(data.workspaceId, data.subjectId)}>← Subject details</a>
  {#if subject}
    <h1 id="conversation-subject-heading">{subject.role}</h1>
    {#if accountConfirmed}
      {#key JSON.stringify([data.accountId, data.workspaceId, subject.id])}
        <SubjectBody accountId={data.accountId} workspaceId={data.workspaceId} subjectId={subject.id} />
        <SubjectConversation accountId={data.accountId} workspaceId={data.workspaceId} subjectId={subject.id} subjectName={subject.role} />
      {/key}
    {:else}
      <p role="alert">The conversation account could not be confirmed. Reopen this page after confirming your session.</p>
    {/if}
  {:else}
    <h1 id="conversation-subject-heading">Subject conversation</h1>
    <p role="alert">{data.subject.error ?? 'The Subject is unavailable. No conversation input was sent.'}</p>
  {/if}
</section>

<style>
  .conversation-page { display: grid; gap: var(--space-4); padding: var(--space-4); width: 100%; box-sizing: border-box; max-width: 960px; min-width: 0; margin-inline: auto; }
  h1 { margin: 0; overflow-wrap: anywhere; }
</style>
