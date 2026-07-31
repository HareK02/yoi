<script lang="ts">
  import type { Snippet } from 'svelte';
  import { getHeaderController } from './context';

  let { content }: { content: Snippet<[]> } = $props();
  const controller = getHeaderController();

  $effect(() => {
    controller.content = content;
    return () => {
      if (controller.content === content) controller.content = null;
    };
  });
</script>
