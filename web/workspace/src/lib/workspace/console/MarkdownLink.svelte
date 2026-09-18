<script lang="ts">
  import type { Snippet } from "svelte";

  let {
    href = "",
    title,
    children,
  }: {
    href?: string;
    title?: string;
    children?: Snippet;
  } = $props();
</script>

<a {href} {title} target="_blank" rel="noreferrer">
  {@render children?.()}
</a>
