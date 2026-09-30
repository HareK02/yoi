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

  let safeHref = $derived(httpHref(href));

  function httpHref(value: string): string | undefined {
    try {
      const protocol = new URL(value).protocol.toLowerCase();
      return protocol === "http:" || protocol === "https:" ? value : undefined;
    } catch {
      return undefined;
    }
  }
</script>

{#if safeHref}
  <a href={safeHref} {title} target="_blank" rel="noreferrer">
    {@render children?.()}
  </a>
{:else}
  {@render children?.()}
{/if}
