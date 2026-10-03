<script lang="ts">
  import { workspaceRoute } from "#lib/workspace/api/http.ts";

  type Props = {
    currentPath?: string;
    workspaceId: string;
  };

  let { currentPath = "/", workspaceId }: Props = $props();
  let subjectsHref = $derived(
    workspaceId ? workspaceRoute(workspaceId, "/memory") : "/",
  );
  let subjectsActive = $derived(
    currentPath === subjectsHref || currentPath.startsWith(`${subjectsHref}/`),
  );
</script>

<section class="sidebar-nav-section sidebar-nav-section--category">
  <h2 class="sidebar-nav-section__header">Memory</h2>

  <a
    class="sidebar-link"
    class:active={subjectsActive}
    href={subjectsHref}
    aria-current={subjectsActive ? "page" : undefined}
  >Subjects</a>
</section>
