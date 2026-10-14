<script lang="ts">
  import DeferredPage from './DeferredPage.svelte';
  import DeferredPageTestContent from './DeferredPageTestContent.svelte';
  import type { DeferredResult } from './deferred.ts';

  let { data, identity, onConstruct }: {
    data: {
      deferredWorkspace?: Promise<DeferredResult<{ workspace: string }>>;
      deferredPage: Promise<DeferredResult<{ label: string; message?: string; serializedError?: boolean }>>;
    };
    identity: string;
    onConstruct?: (label: string) => void;
  } = $props();
</script>

<svelte:boundary>
  <header>Valid Workspace</header>
  <aside aria-label="Workspace sidebar"><a href="/w/team/tickets">Tickets</a></aside>
  <main>
    <DeferredPage {data} {identity} title="Ticket">
      {#snippet children(content)}
        <DeferredPageTestContent {content} {onConstruct} />
      {/snippet}
    </DeferredPage>
  </main>
  {#snippet failed()}
    <h1>Workspace unavailable</h1>
    <p>Workspace removed</p>
    <button type="button">Create Workspace</button>
  {/snippet}
</svelte:boundary>
