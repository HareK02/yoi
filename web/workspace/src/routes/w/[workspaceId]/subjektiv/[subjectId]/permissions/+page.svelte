<script lang="ts">
  import DeferredPage from '#lib/workspace/navigation/DeferredPage.svelte';
  import PageContent from './PageContent.svelte';
  import type { PageProps } from './$types';
  let { data }: PageProps = $props();
</script>
<DeferredPage {data} title="Subject permissions">
  {#snippet children(resolved)}<PageContent data={resolved} />{/snippet}
</DeferredPage>
