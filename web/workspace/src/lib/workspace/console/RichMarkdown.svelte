<script module lang="ts">
  import {
    createShikiHighlighter,
    setShikiHighlighter,
  } from "@humanspeak/svelte-markdown/extensions/shiki";
  import bash from "shiki/langs/bash.mjs";
  import javascript from "shiki/langs/javascript.mjs";
  import json from "shiki/langs/json.mjs";
  import python from "shiki/langs/python.mjs";
  import rust from "shiki/langs/rust.mjs";
  import typescript from "shiki/langs/typescript.mjs";
  import kanagawaWave from "shiki/themes/kanagawa-wave.mjs";

  setShikiHighlighter(
    createShikiHighlighter({
      themes: [kanagawaWave],
      langs: [bash, javascript, json, python, rust, typescript],
    }),
  );
</script>

<script lang="ts">
  import SvelteMarkdown, {
    buildUnsupportedHTML,
    type Renderers,
  } from "@humanspeak/svelte-markdown";
  import { ShikiCode } from "@humanspeak/svelte-markdown/extensions/shiki";
  import MarkdownLink from "$lib/workspace/console/MarkdownLink.svelte";

  type Props = {
    text: string;
    streamId?: string | number;
    class?: string;
  };

  let { text, streamId = "static", class: className = "" }: Props = $props();

  const renderers = {
    code: ShikiCode,
    html: buildUnsupportedHTML(),
    link: MarkdownLink,
  } satisfies Partial<Renderers>;
</script>

<div class={`rich-markdown ${className}`}>
  <SvelteMarkdown source={text} {streamId} {renderers} streaming />
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
