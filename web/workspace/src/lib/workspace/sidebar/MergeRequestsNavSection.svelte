<script lang="ts">
  import { mergeRequestPagePath } from "$lib/workspace/api/merge-requests";

  type Props = {
    currentPath?: string;
    workspaceId: string;
  };

  let { currentPath = "/", workspaceId }: Props = $props();
  let href = $derived(workspaceId ? mergeRequestPagePath(workspaceId) : "/");
</script>

<section class="nav-section">
  <header class="section-header"><span>Merge Requests</span></header>
  <a class="objective-link" class:active={currentPath.startsWith(href)} {href}>
    <span class="item-title">All Merge Requests</span>
    <span class="item-meta">review and integration resources</span>
  </a>
</section>
