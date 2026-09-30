<script lang="ts">
  import type { Renderers } from "@humanspeak/svelte-markdown";
  import DocumentCodeBlock from "./DocumentCodeBlock.svelte";
  import MarkdownTable from "./MarkdownTable.svelte";
  import SafeMarkdown from "./SafeMarkdown.svelte";

  let { text }: { text: string } = $props();

  const renderers = {
    code: DocumentCodeBlock,
    table: MarkdownTable,
  } satisfies Partial<Renderers>;
</script>

<SafeMarkdown {text} class="document-markdown" {renderers} />

<style>
  :global(.document-markdown) {
    min-width: 0;
    max-width: 100%;
    color: var(--text);
    font-size: var(--font-size-body);
    line-height: var(--line-height-body);
    overflow-wrap: anywhere;
  }

  :global(.document-markdown > :first-child) {
    margin-top: 0;
  }

  :global(.document-markdown > :last-child) {
    margin-bottom: 0;
  }

  :global(.document-markdown h1) {
    margin: var(--space-6) 0 var(--space-4);
    color: var(--text-strong);
    font-size: var(--font-size-title);
    line-height: var(--line-height-title);
  }

  :global(.document-markdown h2),
  :global(.document-markdown h3),
  :global(.document-markdown h4),
  :global(.document-markdown h5),
  :global(.document-markdown h6) {
    margin: var(--space-6) 0 var(--space-2);
    color: var(--text-strong);
    font-size: var(--font-size-body);
    line-height: var(--line-height-body);
  }

  :global(.document-markdown h2) {
    font-weight: 700;
  }

  :global(.document-markdown h3),
  :global(.document-markdown h4),
  :global(.document-markdown h5),
  :global(.document-markdown h6) {
    font-weight: 600;
  }

  :global(.document-markdown p),
  :global(.document-markdown ul),
  :global(.document-markdown ol),
  :global(.document-markdown blockquote),
  :global(.document-markdown .markdown-code-region),
  :global(.document-markdown .markdown-table-region) {
    margin: 0 0 var(--space-4);
  }

  :global(.document-markdown ul),
  :global(.document-markdown ol) {
    padding-left: var(--space-5);
  }

  :global(.document-markdown li + li) {
    margin-top: var(--space-1);
  }

  :global(.document-markdown blockquote) {
    margin-left: 0;
    padding-left: var(--space-4);
    border-left: 2px solid var(--line-strong);
    color: var(--text-muted);
  }

  :global(.document-markdown a) {
    color: var(--accent);
    text-underline-offset: 0.18em;
  }

  :global(.document-markdown :not(pre) > code) {
    padding: 0 var(--space-1);
    background: var(--bg-subtle);
    color: var(--code);
    overflow-wrap: anywhere;
  }

  :global(.document-markdown .markdown-code-region),
  :global(.document-markdown .markdown-table-region) {
    max-width: 100%;
    overflow-x: auto;
    overscroll-behavior-inline: contain;
  }

  :global(.document-markdown .markdown-code-region) {
    background: var(--bg-raised);
    color: var(--code);
  }

  :global(.document-markdown .markdown-code-region pre) {
    width: max-content;
    min-width: 100%;
    margin: 0;
    padding: var(--space-4);
    background: transparent;
    color: inherit;
    white-space: pre;
  }

  :global(.document-markdown .markdown-table-region table) {
    width: max-content;
    min-width: 100%;
    border-collapse: collapse;
  }

  :global(.document-markdown .markdown-table-region th),
  :global(.document-markdown .markdown-table-region td) {
    padding: var(--space-2) var(--space-3);
    border-bottom: 1px solid var(--line);
    text-align: left;
    vertical-align: top;
    white-space: nowrap;
  }

  :global(.document-markdown .markdown-table-region th) {
    color: var(--text-strong);
    font-weight: 600;
  }

  :global(.document-markdown hr) {
    margin: var(--space-6) 0;
    border: 0;
    border-top: 1px solid var(--line);
  }
</style>
