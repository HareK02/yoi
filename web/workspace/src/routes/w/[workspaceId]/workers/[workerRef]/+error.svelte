<script lang="ts">
  import { page } from "$app/state";
  import { workspaceRoute } from "#lib/workspace/api/http.ts";
</script>

<svelte:head><title>Worker page error · Yoi</title></svelte:head>

<section class="card" role="alert">
  <h1>Unable to open this Worker page</h1>
  <p>The Worker page could not be loaded or displayed. This does not mean the Workspace is unavailable.</p>
  <p>Status: {page.status}</p>
  <button type="button" class="secondary-button" onclick={() => window.location.reload()}>Reload page</button>
  <a href={workspaceRoute(page.params.workspaceId ?? "", "/workers")}>Back to Workers</a>
</section>
