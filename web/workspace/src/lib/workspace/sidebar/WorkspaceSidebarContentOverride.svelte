<script lang="ts">
  import { getContext, type Snippet } from 'svelte';
  import {
    WORKSPACE_SIDEBAR_CONTENT_CONTEXT,
    type WorkspaceSidebarContentController,
  } from './workspace-content-context';

  let { content }: { content: Snippet } = $props();

  const controller = getContext<WorkspaceSidebarContentController>(
    WORKSPACE_SIDEBAR_CONTENT_CONTEXT,
  );

  $effect(() => controller.registerContent(content));
</script>
