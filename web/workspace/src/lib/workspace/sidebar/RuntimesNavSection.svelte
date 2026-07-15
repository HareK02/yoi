<script lang="ts">
  type Props = {
    currentPath?: string;
    workspaceId: string;
  };

  let { currentPath = '/', workspaceId }: Props = $props();
  let runtimesHref = $derived(workspaceId ? `/w/${workspaceId}/runtimes` : '/runtimes');
  let active = $derived(currentPath === runtimesHref || currentPath.startsWith(`${runtimesHref}/`));
</script>

<section class="sidebar-section" aria-labelledby="runtimes-heading">
  <div class="section-heading-row">
    <h2 id="runtimes-heading">
      <a
        class="section-heading-link"
        class:active
        href={runtimesHref}
        aria-current={active ? 'page' : undefined}
      >runtimes</a>
    </h2>
  </div>
</section>
