<script lang="ts">
  import { ansiSegments } from "./ansi.ts";

  let { text }: { text: string } = $props();
  let segments = $derived(ansiSegments(text));
</script>

{#each segments as segment}
  <span
    class:ansi-bold={segment.bold}
    class:ansi-dim={segment.dim}
    class:ansi-italic={segment.italic}
    class:ansi-underline={segment.underline}
    class:ansi-strikethrough={segment.strikethrough}
    class:ansi-concealed={segment.concealed}
    style:color={segment.foreground}
    style:background-color={segment.background}
  >{segment.text}</span>
{/each}

<style>
  .ansi-bold {
    font-weight: 700;
  }

  .ansi-dim {
    opacity: 0.65;
  }

  .ansi-italic {
    font-style: italic;
  }

  .ansi-underline {
    text-decoration-line: underline;
  }

  .ansi-strikethrough {
    text-decoration-line: line-through;
  }

  .ansi-underline.ansi-strikethrough {
    text-decoration-line: underline line-through;
  }

  .ansi-concealed {
    visibility: hidden;
  }
</style>
