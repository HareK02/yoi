<script lang="ts">
  import { page } from '$app/state';
  import { buildWorkspaceBreadcrumbs } from './breadcrumb-model';

  let { workspaceId }: { workspaceId: string } = $props();

  const workerName = $derived.by(() => {
    const data = page.data as Record<string, unknown>;
    const worker = data.worker as { display_name?: string | null; label?: string | null } | null | undefined;
    return worker?.display_name ?? worker?.label ?? null;
  });
  const breadcrumbs = $derived(buildWorkspaceBreadcrumbs(page.url.pathname, workspaceId, { workerName }));
  const workspaceRoot = $derived(`/w/${encodeURIComponent(workspaceId)}`);
</script>

<nav class="workspace-breadcrumbs" aria-label="Current workspace location">
  <a class="workspace-breadcrumb-root" href={workspaceRoot} aria-label="Workspace home">/</a>
  {#each breadcrumbs as breadcrumb, index (`${index}:${breadcrumb.label}`)}
    {#if index > 0}<span class="workspace-breadcrumb-separator" aria-hidden="true">/</span>{/if}
    {#if breadcrumb.href}
      <a href={breadcrumb.href}>{breadcrumb.label}</a>
    {:else}
      <span class="workspace-breadcrumb-label" aria-current={index === breadcrumbs.length - 1 ? 'page' : undefined}>
        {breadcrumb.label}
      </span>
    {/if}
  {/each}
</nav>

<style>
  .workspace-breadcrumbs {
    display: flex;
    min-width: 0;
    align-items: center;
    gap: 0.55rem;
    color: var(--workspace-muted, #53606e);
    font-family: var(--workspace-font-mono, monospace);
    font-size: 0.84rem;
    line-height: 1;
  }

  .workspace-breadcrumbs a,
  .workspace-breadcrumb-label {
    max-width: min(32vw, 28rem);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .workspace-breadcrumbs a {
    color: inherit;
    text-decoration: none;
  }

  .workspace-breadcrumbs a:hover {
    color: var(--workspace-ink, #151b23);
    text-decoration: underline;
    text-underline-offset: 0.22rem;
  }

  .workspace-breadcrumb-root {
    font-weight: 700;
  }

  .workspace-breadcrumb-separator {
    color: color-mix(in srgb, currentColor 45%, transparent);
    user-select: none;
  }

  .workspace-breadcrumbs span[aria-current='page'] {
    color: var(--workspace-ink, #151b23);
    font-weight: 600;
  }
</style>
