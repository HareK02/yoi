<script lang="ts">
  import type { Snippet } from "svelte";

  let { children }: { children?: Snippet } = $props();
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex (horizontal scroll regions must be keyboard-focusable) -->
<div class="markdown-table-region" role="region" aria-label="Markdown table" tabindex="0">
  <table>{@render children?.()}</table>
</div>
