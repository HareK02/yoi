<script lang="ts">
  import { getContext, type Snippet } from 'svelte';
  import { markdownLinkTarget, MARKDOWN_WORKSPACE_CONTEXT, type MarkdownWorkspace } from './link-policy.ts';
  const workspace = getContext<MarkdownWorkspace | undefined>(MARKDOWN_WORKSPACE_CONTEXT);
  let { href = '', title, children }: { href?: string; title?: string; children?: Snippet } = $props();
  let target = $derived(markdownLinkTarget(href, workspace?.()));
</script>
{#if target}
  <a href={target.href} {title} target={target.external ? '_blank' : undefined} rel={target.external ? 'noreferrer' : undefined}>
    {@render children?.()}
  </a>
{:else}
  {@render children?.()}
{/if}
