<script lang="ts">
  import { page } from "$app/state";
  import "#lib/workspace/styles/workspace-pages.css";
</script>

<svelte:head><title>Page error · Yoi</title></svelte:head>

<section class="workspace-page">
  <h1>Unable to display this page</h1>
  <p role="alert">{page.error?.message ?? "An unexpected error occurred."}</p>
  <p>Error {page.status}</p>
  <div class="workspace-toolbar">
    <button type="button" class="workspace-secondary-button" onclick={() => window.location.reload()}>Reload page</button>
    <a href="/">Home</a>
  </div>
</section>
