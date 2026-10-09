<script lang="ts">
  import { workspaceRoute } from '#lib/workspace/api/http.ts';
  let { workspaceId, currentPath = '/' }: { workspaceId: string; currentPath?: string } = $props();
  let href = $derived(workspaceRoute(workspaceId, '/drive'));
  let active = $derived(currentPath === href || currentPath.startsWith(`${href}/`));
</script>
<section class="sidebar-nav-section sidebar-nav-section--category">
  <a class="sidebar-link" class:active {href} aria-current={active ? 'page' : undefined}>Drive</a>
</section>
