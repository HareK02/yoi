<script lang="ts">
  import { workspaceRoute } from '$lib/workspace/api/http';

  type Props = {
    currentPath?: string;
    workspaceId: string;
  };

  let { currentPath = '/', workspaceId }: Props = $props();
  let objectivesHref = $derived(workspaceId ? workspaceRoute(workspaceId, '/objectives') : '/objectives');
</script>

<section class="sidebar-nav-section sidebar-nav-section--resource">
  <a
    class="sidebar-link"
    class:active={currentPath.startsWith(objectivesHref)}
    href={objectivesHref}
    aria-current={currentPath.startsWith(objectivesHref) ? 'page' : undefined}
  >Objectives</a>
</section>
