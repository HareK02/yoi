<script lang="ts">
  import { mergeRequestPagePath } from "$lib/workspace/api/merge-requests";

  type Props = {
    currentPath?: string;
    workspaceId: string;
  };

  let { currentPath = "/", workspaceId }: Props = $props();
  let href = $derived(workspaceId ? mergeRequestPagePath(workspaceId) : "/");
</script>

<section class="sidebar-nav-section sidebar-nav-section--resource">
  <a
    class="sidebar-link"
    class:active={currentPath.startsWith(href)}
    {href}
    aria-current={currentPath.startsWith(href) ? "page" : undefined}
  >Merge Requests</a>
</section>
