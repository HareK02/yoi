<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import { legacySubjectDestination, subjectsHref } from './routes.ts';

  const destination = $derived(legacySubjectDestination(page.url));
  let navigationFailed = $state(false);

  // static SPAではHTTP redirectを返せない。旧entryを置換し、戻るで往復させない。
  // hashはloadではなくブラウザのpage.urlから引き継ぐ。
  $effect(() => {
    const target = destination;
    navigationFailed = false;
    if (target) void goto(target, { replaceState: true }).catch(() => {
      navigationFailed = true;
    });
  });
</script>

<svelte:head><title>Subject URL moved · Yoi Workspace</title></svelte:head>
<section class="legacy-subject-route">
  <h1>Subject URL moved</h1>
  {#if destination}
    <p>{navigationFailed ? 'Automatic navigation failed.' : 'Opening the new Subject URL…'}</p>
    <a href={destination}>Continue to the new URL →</a>
  {:else}
    <p>This legacy path has no corresponding Subject page.</p>
    <a href={subjectsHref(page.params.workspaceId ?? '')}>Open Subjects →</a>
  {/if}
</section>

<style>
  .legacy-subject-route { padding: var(--space-4); }
</style>
