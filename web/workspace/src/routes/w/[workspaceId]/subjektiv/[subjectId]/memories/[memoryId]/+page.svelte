<script lang="ts">
  import DeferredPage from '#lib/workspace/navigation/DeferredPage.svelte';
  import ReadingPosition from '#lib/workspace/memory/ReadingPosition.svelte';
  import PageContent from './PageContent.svelte';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
  let position = $state<{ top: number } | null>(null);
  export const snapshot = {
    capture: () => ({ top: document.querySelector('main')?.scrollTop ?? 0 }),
    restore: (saved: { top: number }) => { position = saved; },
  };
</script>

<DeferredPage {data} title="Memory">
  {#snippet children(resolved)}
    <PageContent data={resolved} />
    <ReadingPosition {position} restored={() => { position = null; }} />
  {/snippet}
</DeferredPage>
