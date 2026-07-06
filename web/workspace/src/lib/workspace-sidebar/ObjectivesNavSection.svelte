<script lang="ts">
  import { workspaceRoute } from '$lib/workspace-api/http';

  type Props = {
    currentPath?: string;
    workspaceId: string;
  };

  let { currentPath = '/', workspaceId }: Props = $props();
  let objectivesHref = $derived(workspaceId ? workspaceRoute(workspaceId, '/objectives') : '/objectives');
</script>

<section class="nav-section">
  <header class="section-header">
    <span>Objectives</span>
  </header>

  <a class="objective-link" class:active={currentPath.startsWith(objectivesHref)} href={objectivesHref}>
    <span class="item-title">Open Objectives</span>
    <span class="item-meta">workspace objectives</span>
  </a>
</section>
