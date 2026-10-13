<script lang="ts">
  import type { PageProps } from './$types';
  import DeferredPage from '#lib/workspace/navigation/DeferredPage.svelte';
  import PageContent from './PageContent.svelte';

  let { data }: PageProps = $props();
</script>

<DeferredPage {data} title="Repository Access">
  {#snippet children(resolved)}
    <PageContent data={resolved} />
  {/snippet}
</DeferredPage>
