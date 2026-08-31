<script lang="ts">
  import { page } from '$app/state';
  import { workspaceRoute } from '$lib/workspace/api/http';

  let settingsHref = $derived(
    page.params.workspaceId
      ? workspaceRoute(page.params.workspaceId, '/settings')
      : '/settings',
  );
</script>

<svelte:head>
  <title>Settings unavailable · Yoi Workspace</title>
</svelte:head>

<section class="settings-panel" aria-labelledby="settings-error-title">
  <p class="eyebrow">Settings</p>
  <h1 id="settings-error-title">This settings page could not be loaded</h1>
  <p class="section-state error">
    {page.error?.message ?? 'The Backend rejected or could not complete the settings request.'}
  </p>
  <div class="settings-action-row">
    <a class="button-link" href={settingsHref}>Back to Settings</a>
  </div>
</section>
