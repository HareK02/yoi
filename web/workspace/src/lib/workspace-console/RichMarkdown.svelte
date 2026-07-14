<script lang="ts">
  import { markdownToHtml } from "$lib/workspace-console/markdown";

  type Props = {
    text: string;
    class?: string;
  };

  let { text, class: className = "" }: Props = $props();
  let html = $state("");
  let rendering = $state(false);

  async function render(value: string): Promise<void> {
    const current = value;
    rendering = true;
    try {
      const next = await markdownToHtml(current);
      if (text === current) {
        html = next;
      }
    } finally {
      if (text === current) {
        rendering = false;
      }
    }
  }

  $effect(() => {
    void render(text);
  });
</script>

<div class={`rich-markdown ${className}`} class:is-rendering={rendering}>
  {#if html}
    {@html html}
  {:else}
    <p>{text}</p>
  {/if}
</div>

<style>
  .rich-markdown {
    color: inherit;
    line-height: 1.55;
  }

  :global(.rich-markdown > :first-child) {
    margin-top: 0;
  }

  :global(.rich-markdown > :last-child) {
    margin-bottom: 0;
  }

  :global(.rich-markdown p),
  :global(.rich-markdown ul),
  :global(.rich-markdown blockquote),
  :global(.rich-markdown pre) {
    margin: 0.45rem 0;
  }

  :global(.rich-markdown ul) {
    padding-left: 1.2rem;
  }

  :global(.rich-markdown h1),
  :global(.rich-markdown h2),
  :global(.rich-markdown h3),
  :global(.rich-markdown h4) {
    margin: 0.7rem 0 0.35rem;
    color: var(--text-strong);
    font-size: 1rem;
  }

  :global(.rich-markdown blockquote) {
    border-left: 2px solid var(--tui-dark-gray);
    color: var(--text-muted);
    padding-left: 0.8rem;
  }

  :global(.rich-markdown :not(pre) > code) {
    background: color-mix(in oklch, var(--bg-raised) 80%, var(--tui-blue));
    border: 1px solid var(--line);
    border-radius: 0.35rem;
    color: var(--tui-blue);
    padding: 0.05rem 0.28rem;
  }

  :global(.rich-markdown a) {
    color: var(--tui-cyan);
  }

  :global(.rich-markdown .shiki) {
    border: 1px solid var(--line);
    border-radius: 0.65rem;
    overflow: auto;
    padding: 0.75rem;
  }
</style>
