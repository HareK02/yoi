<script lang="ts">
  import { workspaceRoute } from '$lib/workspace/api/http';

  type Props = {
    currentPath?: string;
    workspaceId: string;
  };

  let { currentPath = '/', workspaceId }: Props = $props();
  let ticketsHref = $derived(workspaceId ? workspaceRoute(workspaceId, '/tickets') : '/');
</script>

<section class="sidebar-nav-section sidebar-nav-section--resource">
  <a
    class="sidebar-link"
    class:active={currentPath.startsWith(ticketsHref)}
    href={ticketsHref}
    aria-current={currentPath.startsWith(ticketsHref) ? 'page' : undefined}
  >Tickets</a>
</section>
