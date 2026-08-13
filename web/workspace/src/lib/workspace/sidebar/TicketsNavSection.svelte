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
  <a
    class="primary-nav-link"
    class:active={currentPath.startsWith(ticketsHref)}
    href={ticketsHref}
    aria-current={currentPath.startsWith(ticketsHref) ? 'page' : undefined}
  >Tickets</a>
</section>
