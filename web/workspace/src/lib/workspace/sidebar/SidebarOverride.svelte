<script lang="ts">
  import {
    getSidebarController,
    type SidebarController,
    type SidebarSnippet,
  } from './context';

  type Props = {
    sidebar: SidebarSnippet;
    controller?: SidebarController;
  };

  const inheritedController = getSidebarController();
  const { sidebar, controller = inheritedController }: Props = $props();

  $effect(() => controller.registerSidebar(sidebar));
</script>
