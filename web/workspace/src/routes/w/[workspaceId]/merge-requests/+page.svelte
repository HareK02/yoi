<script lang="ts">
  import PageContent from "./PageContent.svelte";
  import DeferredPage from "#lib/workspace/navigation/DeferredPage.svelte";
  import type { PageData } from "./$types";

  let { data }: { data: PageData } = $props();
</script>

<DeferredPage {data} title="Merge Requests">
  {#snippet children(resolved)}
    <PageContent data={resolved} />
  {/snippet}
</DeferredPage>
