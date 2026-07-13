<script lang="ts">
  import RichMarkdown from '$lib/workspace-console/RichMarkdown.svelte';
  import type { ConsoleLine } from '$lib/workspace-console/model';

  type Props = {
    item: ConsoleLine;
  };

  let { item }: Props = $props();

  function lineClass(line: ConsoleLine): string {
    return line.error ? 'error' : line.kind;
  }

  function toolClass(line: ConsoleLine): string {
    const name = line.toolCall?.name?.toLowerCase() ?? '';
    const state = line.toolCall?.state ?? (line.streaming ? 'streaming' : 'done');
    return [name ? `tool-${name}` : '', `tool-state-${state}`].filter(Boolean).join(' ');
  }

  function shouldRenderHeading(line: ConsoleLine): boolean {
    return line.kind !== 'assistant' && line.kind !== 'user' && line.kind !== 'tool';
  }

  function toolSummary(line: ConsoleLine): { label: string; suffix: string; rest: string } {
    const [firstLine = '', ...rest] = line.body.split('\n');
    const [label, suffix = ''] = firstLine.split(' — ', 2);
    return {
      label,
      suffix,
      rest: rest.join('\n')
    };
  }

  function shouldRenderMarkdown(line: ConsoleLine): boolean {
    return line.kind === 'user' || line.kind === 'assistant' || line.kind === 'system';
  }

  function bodyTextAfterToolSummary(line: ConsoleLine): string {
    return toolSummary(line).rest;
  }
</script>

<li class={`console-line ${lineClass(item)} ${toolClass(item)}`} class:error-line={item.error}>
  {#if shouldRenderHeading(item)}
    <div class="message-heading">
      <span>{item.title}</span>
      {#if item.streaming}<small>streaming</small>{/if}
    </div>
  {:else if item.kind === 'tool'}
    <div class="tool-summary">
      <span class="tool-label">{toolSummary(item).label}</span>
      <span class="tool-separator"> — </span>
      <span class={`tool-suffix ${item.toolCall?.state ?? ''}`}>{toolSummary(item).suffix}</span>
      {#if item.streaming}<small>streaming</small>{/if}
    </div>
  {:else if item.streaming}
    <div class="message-heading streaming-heading">
      <small>streaming</small>
    </div>
  {/if}
  {#if item.kind === 'tool'}
    {#if bodyTextAfterToolSummary(item)}
      <p class="console-plain-text">{bodyTextAfterToolSummary(item)}</p>
    {/if}
  {:else if shouldRenderMarkdown(item)}
    <RichMarkdown text={item.body || '—'} />
  {:else}
    <p class="console-plain-text">{item.body || '—'}</p>
  {/if}
  {#if item.diff}
    <pre class="console-diff" aria-label="Edit diff">{#each item.diff as diffLine}
<span class={`diff-line ${diffLine.kind}`}><span class="diff-gutter">{diffLine.oldNumber ?? ''}</span><span class="diff-gutter">{diffLine.newNumber ?? ''}</span><span class="diff-marker">{diffLine.kind === 'add' ? '+' : diffLine.kind === 'remove' ? '-' : ' '}</span><span class="diff-content">{diffLine.content}</span></span>{/each}</pre>
  {/if}
  {#if item.detail}
    <details class="message-detail">
      <summary>detail</summary>
      <p>{item.detail}</p>
    </details>
  {/if}
</li>
