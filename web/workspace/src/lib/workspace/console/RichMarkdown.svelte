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
  import type { Renderers } from "@humanspeak/svelte-markdown";
  import { ShikiCode } from "@humanspeak/svelte-markdown/extensions/shiki";
  import SafeMarkdown from "#lib/workspace/markdown/SafeMarkdown.svelte";

  type Props = {
    text: string;
    streamId?: string | number;
    class?: string;
  };

  let { text, streamId = "static", class: className = "" }: Props = $props();

  const renderers = {
    code: ShikiCode,
  } satisfies Partial<Renderers>;
</script>

<SafeMarkdown {text} {streamId} class={`rich-markdown ${className}`} {renderers} streaming />

<style>
  :global(.rich-markdown) {
    color: inherit;
    line-height: var(--line-height-body);
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
    margin: var(--space-2) 0;
  }

  :global(.rich-markdown ul) {
    padding-left: var(--space-4);
  }

  :global(.rich-markdown h1),
  :global(.rich-markdown h2),
  :global(.rich-markdown h3),
  :global(.rich-markdown h4) {
    margin: var(--space-3) 0 var(--space-1);
    color: var(--text-strong);
    font-size: var(--font-size-body);
  }

  :global(.rich-markdown blockquote) {
    border-left: 2px solid var(--tui-dark-gray);
    color: var(--text-muted);
    padding-left: var(--space-3);
  }

  :global(.rich-markdown :not(pre) > code) {
    background: color-mix(in oklch, var(--bg-raised) 80%, var(--tui-blue));
    border: 1px solid var(--line);
    border-radius: 0.35rem;
    color: var(--tui-blue);
    padding: 0 var(--space-1);
  }

  :global(.rich-markdown a) {
    color: var(--tui-cyan);
  }

  :global(.rich-markdown .shiki),
  :global(.rich-markdown .shiki-fallback) {
    border: 1px solid var(--line);
    border-radius: 0.65rem;
    overflow: auto;
    padding: var(--space-3);
  }
</style>
