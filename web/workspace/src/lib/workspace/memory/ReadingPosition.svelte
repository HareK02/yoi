<script lang="ts">
  import { tick } from 'svelte';
  let { position, restored }: { position: { top: number } | null; restored: () => void } = $props();
  // mainがscroll owner。deferred本文が描画される前のrestoreでは高さが足りず位置を失う。
  $effect(() => {
    if (!position) return;
    const top = position.top;
    let cancelled = false;
    void tick().then(() => requestAnimationFrame(() => {
      if (cancelled) return;
      document.querySelector('main')?.scrollTo({ top, behavior: 'instant' });
      restored();
    }));
    return () => { cancelled = true; };
  });
</script>
