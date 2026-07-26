<script lang="ts">
  import { workspaceRoute } from '$lib/workspace/api/http';

  type Props = {
    currentPath?: string;
    workspaceId: string;
  };

  let { currentPath = '/', workspaceId }: Props = $props();
  let documentHref = $derived(workspaceId ? workspaceRoute(workspaceId, '/memory') : '/');
  let stagingHref = $derived(workspaceId ? workspaceRoute(workspaceId, '/memory/staging') : '/');
</script>

<section class="nav-section">
  <header class="section-header">
    <span>Memory</span>
  </header>

  <a class="objective-link" class:active={currentPath === documentHref} href={documentHref}>
    <span class="item-title">Document</span>
    <span class="item-meta">durable workspace memory</span>
  </a>

  <a class="objective-link" class:active={currentPath.startsWith(stagingHref)} href={stagingHref}>
    <span class="item-title">Staging</span>
    <span class="item-meta">pending extraction candidates</span>
  </a>
</section>
