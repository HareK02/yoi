<script lang="ts">
  import { subjectsHref } from '#lib/workspace/subjektiv/routes.ts';
  import { ownsRoutePath } from './route-ownership.ts';

  let { workspaceId, currentPath = '/' }: { workspaceId: string; currentPath?: string } = $props();
  const href = $derived(subjectsHref(workspaceId));
  const active = $derived(ownsRoutePath(href, currentPath));
</script>

<section class="sidebar-nav-section sidebar-nav-section--resource">
  <a class="sidebar-link" class:active {href} aria-current={active ? 'page' : undefined}>Subjects</a>
</section>
