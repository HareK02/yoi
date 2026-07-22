<script lang="ts">
  import { workspaceRoute } from '$lib/workspace/api/http';

  type Props = {
    currentPath?: string;
    workspaceId: string;
  };

  let { currentPath = '/', workspaceId }: Props = $props();
  let ticketsHref = $derived(workspaceId ? workspaceRoute(workspaceId, '/tickets') : '/');
</script>

<section class="nav-section">
  <header class="section-header">
    <span>Tickets</span>
  </header>

  <a class="objective-link" class:active={currentPath.startsWith(ticketsHref)} href={ticketsHref}>
    <span class="item-title">Open Tickets</span>
    <span class="item-meta">workspace tickets</span>
  </a>
</section>
