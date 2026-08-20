<script lang="ts">
  import RichMarkdown from '$lib/workspace/console/RichMarkdown.svelte';
  import type { ConsoleLine } from '$lib/workspace/console/model';

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
    return line.kind !== 'assistant' && line.kind !== 'user' && line.kind !== 'tool' &&
      line.kind !== 'activity' && line.kind !== 'task_reminder' && line.kind !== 'run_stats';
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

<li
  class={`console-line ${lineClass(item)} ${toolClass(item)}`}
  class:error-line={item.error}
  data-console-line-id={item.id}
>
  {#if shouldRenderHeading(item)}
    <div class="message-heading">
      <span>{item.title}</span>
    </div>
  {:else if item.kind === 'tool'}
    <div class="tool-summary">
      <span class="tool-label">{toolSummary(item).label}</span>
      <span class="tool-separator"> — </span>
      <span class={`tool-suffix ${item.toolCall?.state ?? ''}`}>{toolSummary(item).suffix}</span>
    </div>
  {/if}
  {#if item.kind === 'tool'}
    {#if bodyTextAfterToolSummary(item)}
      <p class="console-plain-text">{bodyTextAfterToolSummary(item)}</p>
    {/if}
  {:else if item.kind === 'user'}
    <div class="user-message">
      <span class="user-prompt" aria-hidden="true">&gt;</span>
      <div><RichMarkdown text={item.body || '—'} /></div>
    </div>
  {:else if item.kind === 'activity'}
    <p class="activity-summary">{item.body || '—'}</p>
  {:else if item.kind === 'task_reminder'}
    <p class="task-reminder-summary">{item.body || 'task reminder'}</p>
  {:else if item.kind === 'run_stats'}
    <p class="run-stats">{item.body}</p>
  {:else if shouldRenderMarkdown(item)}
    <RichMarkdown text={item.body || '—'} />
  {:else}
    <p class="console-plain-text">{item.body || '—'}</p>
  {/if}
  {#if item.diff}
    <div class="console-diff" role="group" aria-label="Edit diff">
      {#each item.diff as diffLine}
        <span class={`diff-line ${diffLine.kind}`}><span class="diff-gutter">{diffLine.oldNumber ?? ''}</span><span class="diff-gutter">{diffLine.newNumber ?? ''}</span><span class="diff-marker">{diffLine.kind === 'add' ? '+' : diffLine.kind === 'remove' ? '-' : ' '}</span><span class="diff-content">{diffLine.content}</span></span>
      {/each}
    </div>
  {/if}
  {#if item.detail}
    <details class="message-detail">
      <summary>detail</summary>
      <p>{item.detail}</p>
    </details>
  {/if}
</li>

<style>
  .console-line {
    min-width: 0;
    padding: 0.2rem 0;
    background: transparent;
  }

  .console-line.error-line {
    color: var(--danger);
  }

  .console-line.user {
    color: var(--tui-green);
  }

  .user-message {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 0.55rem;
    align-items: start;
  }

  .user-prompt {
    color: var(--tui-green);
    font-weight: 700;
    line-height: 1.55;
  }

  .activity-summary,
  .task-reminder-summary {
    margin: 0;
    color: var(--text-muted);
    font-size: 0.78rem;
    line-height: 1.55;
    white-space: pre-line;
  }

  .task-reminder-summary {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .console-line.error .activity-summary {
    color: var(--tui-error);
  }

  .run-stats {
    margin: 0;
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: 0.72rem;
    font-variant-numeric: tabular-nums;
    text-align: right;
    white-space: nowrap;
  }

  .console-line.assistant {
    color: var(--text-strong);
  }

  .console-line.thinking .message-heading {
    color: var(--tui-magenta);
    font-style: italic;
  }

  .console-line.thinking .console-plain-text {
    color: var(--tui-dark-gray);
    font-style: italic;
  }

  .console-plain-text {
    margin: 0.45rem 0;
    color: inherit;
    line-height: 1.55;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }

  .console-plain-text:first-child {
    margin-top: 0;
  }

  .console-plain-text:last-child {
    margin-bottom: 0;
  }

  .console-line.tool-bash .console-plain-text {
    display: block;
    max-width: 100%;
    min-width: 0;
    font-family: var(--font-mono);
    overflow-x: auto;
    white-space: pre;
  }

  .console-line.tool .console-plain-text {
    color: var(--tui-gray);
  }

  .tool-summary {
    display: flex;
    align-items: baseline;
    gap: 0;
    color: var(--text-muted);
    font-size: 0.88rem;
    font-weight: 750;
  }

  .tool-label {
    flex: 0 0 auto;
    color: var(--tui-cyan);
    white-space: nowrap;
  }

  .tool-separator {
    flex: 0 0 auto;
    white-space: nowrap;
  }

  .tool-suffix {
    flex: 1 1 auto;
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .tool-separator,
  .tool-suffix {
    color: var(--tui-dark-gray);
  }

  .tool-state-error .tool-suffix {
    color: var(--tui-red);
  }

  .tool-state-running .tool-suffix,
  .tool-state-streaming_args .tool-suffix,
  .tool-state-pending .tool-suffix {
    color: var(--tui-yellow);
  }

  .tool-state-done .tool-suffix {
    color: var(--tui-dark-gray);
  }

  .message-heading {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
    color: var(--text-muted);
    font-size: 0.78rem;
    font-weight: 750;
  }

  .console-diff {
    background: color-mix(in oklch, var(--bg-raised) 85%, black);
    border: 1px solid var(--line);
    border-radius: 0.65rem;
    color: var(--text);
    font-family: var(--font-mono);
    font-size: 0.78rem;
    line-height: 1.45;
    margin: 0.6rem 0 0;
    overflow-x: auto;
    padding: 0.45rem 0;
  }

  .diff-line {
    display: grid;
    grid-template-columns: 3.2rem 3.2rem 1.4rem minmax(0, 1fr);
    min-width: max-content;
  }

  .diff-line.add {
    background: color-mix(in oklch, var(--tui-green) 18%, transparent);
    color: color-mix(in oklch, var(--tui-green) 75%, white);
  }

  .diff-line.remove {
    background: color-mix(in oklch, var(--tui-red) 18%, transparent);
    color: color-mix(in oklch, var(--tui-red) 72%, white);
  }

  .diff-line.context {
    color: var(--text-muted);
  }

  .diff-gutter,
  .diff-marker {
    color: var(--tui-gray);
    padding: 0 0.5rem;
    text-align: right;
    user-select: none;
  }

  .diff-content {
    padding-right: 0.75rem;
    white-space: pre;
  }

  :global(.console-line pre) {
    max-width: 100%;
    margin: 0;
    overflow-x: auto;
    white-space: pre-wrap;
    color: var(--code);
  }

  .message-detail {
    color: var(--text-muted);
    font-size: 0.84rem;
  }

  .message-detail summary {
    cursor: pointer;
    font-weight: 800;
  }
</style>
