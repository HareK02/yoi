<script lang="ts">
  import { page } from "$app/state";
  import WorkspaceSwitcher from "$lib/workspace/sidebar/WorkspaceSwitcher.svelte";
  import type { WorkspaceResponse } from "$lib/workspace/sidebar/types";
  import { buildWorkspaceBreadcrumbs } from "./breadcrumb-model";

  type Props = {
    workspaceId: string;
    workspace?: WorkspaceResponse | null;
    workspaceError?: string | null;
  };

  let { workspaceId, workspace = null, workspaceError = null }: Props = $props();

  const workerName = $derived.by(() => {
    const data = page.data as Record<string, unknown>;
    const worker = data.worker as
      | { display_name?: string | null; label?: string | null }
      | null
      | undefined;
    return worker?.display_name ?? worker?.label ?? null;
  });
  const breadcrumbs = $derived(
    buildWorkspaceBreadcrumbs(page.url.pathname, workspaceId, { workerName }),
  );
  const currentWorkspaceName = $derived(
    workspaceError ? workspaceId : workspace?.display_name || workspaceId,
  );
</script>

<div class="workspace-header-location">
  <WorkspaceSwitcher
    variant="header"
    currentWorkspaceId={workspaceId}
    {currentWorkspaceName}
  />

  {#if breadcrumbs.length > 0}
    <span class="workspace-breadcrumb-separator" aria-hidden="true">/</span>
    <nav class="workspace-breadcrumbs" aria-label="Current workspace location">
      {#each breadcrumbs as breadcrumb, index (`${index}:${breadcrumb.label}`)}
        {#if index > 0}<span class="workspace-breadcrumb-separator" aria-hidden="true">/</span>{/if}
        {#if breadcrumb.href}
          <a href={breadcrumb.href}>{breadcrumb.label}</a>
        {:else}
          <span class="workspace-breadcrumb-label" aria-current={index === breadcrumbs.length - 1 ? "page" : undefined}>
            {breadcrumb.label}
          </span>
        {/if}
      {/each}
    </nav>
  {/if}
</div>

<style>
  .workspace-header-location,
  .workspace-breadcrumbs {
    display: flex;
    min-width: 0;
    align-items: center;
    gap: 0.55rem;
    color: var(--text-muted);
    font-family: var(--font-mono);
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
    color: var(--text-strong);
    text-decoration: underline;
    text-underline-offset: 0.22rem;
  }

  .workspace-breadcrumb-separator {
    color: color-mix(in srgb, currentColor 45%, transparent);
    user-select: none;
  }

  .workspace-breadcrumbs span[aria-current="page"] {
    color: var(--text-strong);
    font-weight: 600;
  }
</style>
