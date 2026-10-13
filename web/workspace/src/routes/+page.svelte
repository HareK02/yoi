<script lang="ts">
  import DeferredPage from "#lib/workspace/navigation/DeferredPage.svelte";
  import PageContent from "./PageContent.svelte";
  import type { PageData } from "./$types";
  let { data }: { data: PageData } = $props();
</script>

<DeferredPage {data} title="Workspaces">
  {#snippet children(resolved)}<PageContent data={resolved} />{/snippet}
</DeferredPage>
