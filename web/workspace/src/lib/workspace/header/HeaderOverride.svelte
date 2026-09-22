<script lang="ts">
  import type { Snippet } from 'svelte';
  import { getHeaderController } from './context';

  let {
    content,
    active = true,
  }: {
    content: Snippet<[]>;
    active?: boolean;
  } = $props();
  const controller = getHeaderController();

  $effect(() => {
    if (!active) return;
    return controller.registerContent(content);
  });
</script>
