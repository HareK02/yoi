<script lang="ts">
  import { workspaceRoute } from "$lib/workspace/api/http";

  type Props = {
    currentPath?: string;
    workspaceId: string;
  };

  let { currentPath = "/", workspaceId }: Props = $props();
  let documentHref = $derived(workspaceId ? workspaceRoute(workspaceId, "/memory") : "/");
  let stagingHref = $derived(workspaceId ? workspaceRoute(workspaceId, "/memory/staging") : "/");
</script>

<section class="sidebar-nav-section sidebar-nav-section--category">
  <h2 class="sidebar-nav-section__header">Memory</h2>

  <a
    class="sidebar-link"
    class:active={currentPath === documentHref}
    href={documentHref}
    aria-current={currentPath === documentHref ? "page" : undefined}
  >Document</a>

  <a
    class="sidebar-link"
    class:active={currentPath.startsWith(stagingHref)}
    href={stagingHref}
    aria-current={currentPath.startsWith(stagingHref) ? "page" : undefined}
  >Staging</a>
</section>
