<script lang="ts">
  import { getSidebarController, type SidebarSnippet } from './context';

  type Props = {
    sidebar: SidebarSnippet;
  };

  const { sidebar }: Props = $props();
  const controller = getSidebarController();

  $effect(() => controller.registerSidebar(sidebar));
</script>
