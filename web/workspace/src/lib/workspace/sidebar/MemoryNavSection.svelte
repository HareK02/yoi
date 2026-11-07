<script lang="ts">
  import { workspaceRoute } from "#lib/workspace/api/http.ts";
  import { ownsRoutePath } from './route-ownership.ts';

  let { workspaceId, currentPath = '/' }: { workspaceId: string; currentPath?: string } = $props();
  const href = $derived(workspaceRoute(workspaceId, "/memory/staging"));
  const active = $derived(ownsRoutePath(href, currentPath));
</script>

<section class="sidebar-nav-section sidebar-nav-section--category">
  <h2 class="sidebar-nav-section__header">Memory</h2>
  <a class="sidebar-link" class:active {href} aria-current={active ? 'page' : undefined}>Staging</a>
</section>
