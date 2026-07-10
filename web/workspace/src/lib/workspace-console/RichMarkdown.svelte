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
