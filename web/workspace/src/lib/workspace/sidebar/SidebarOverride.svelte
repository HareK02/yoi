<script lang="ts">
  import {
    getSidebarController,
    type SidebarController,
    type SidebarSnippet,
  } from './context';

  type Props = {
    sidebar: SidebarSnippet;
    controller?: SidebarController;
    active?: boolean;
  };

  const inheritedController = getSidebarController();
  const {
    sidebar,
    controller = inheritedController,
    active = true,
  }: Props = $props();

  $effect(() => {
    if (!active) return;
    return controller.registerSidebar(sidebar);
  });
</script>
