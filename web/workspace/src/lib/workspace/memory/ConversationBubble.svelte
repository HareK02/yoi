<script lang="ts">
  import RichMarkdown from '#lib/workspace/console/RichMarkdown.svelte';
  import type { ConversationMessageView } from './conversation-view.ts';
  let { message, onretry, retryDisabled = false }: {
    message: ConversationMessageView;
    onretry?: (operationId: string) => void;
    retryDisabled?: boolean;
  } = $props();
</script>

<li class="chat-message" class:own={message.own} data-message-id={message.id} data-local={message.local || undefined}>
  <div class="chat-bubble">
    <div class="chat-author">{message.author}</div>
    <RichMarkdown text={message.text} streamId={message.id} />
  </div>
  {#if message.own && message.delivery}
    <div class="chat-delivery" aria-live="polite">
      {#if message.delivery.state === 'retry'}
        <button class="retry-delivery" type="button" aria-label="Retry message" title="Retry message"
          disabled={retryDisabled || !onretry} onclick={() => onretry?.(message.delivery!.operationId)}>
          <svg aria-hidden="true" viewBox="0 0 24 24"><path d="M3 11a9 9 0 1 1 2.6 7.4M3 4v7h7" /></svg>
        </button>
      {:else}
        <span>{message.delivery.state}</span>
      {/if}
    </div>
  {/if}
</li>

<style>
  .chat-message { display: flex; flex-direction: column; align-items: flex-start; min-width: 0; }
  .chat-message.own { align-items: flex-end; }
  .chat-bubble { min-width: 0; max-width: min(85%, 48rem); padding: var(--space-3) var(--space-4); background: var(--bg-subtle); border-radius: var(--space-3); overflow-wrap: anywhere; }
  .own .chat-bubble { background: var(--bg-raised); }
  .chat-author { font-size: var(--font-size-compact); line-height: var(--line-height-compact); color: var(--text-muted); margin-bottom: var(--space-1); }
  .chat-delivery { margin-top: var(--space-1); color: var(--text-muted); font-size: var(--font-size-compact); line-height: var(--line-height-compact); }
  .retry-delivery { display: flex; align-items: center; justify-content: center; width: 2rem; height: 2rem; padding: var(--space-1); border: 0; border-radius: var(--radius-soft); background: transparent; color: var(--danger); cursor: pointer; }
  .retry-delivery:hover:not(:disabled) { background: var(--interactive-hover); }
  .retry-delivery:active:not(:disabled) { background: var(--interactive-selected); }
  .retry-delivery:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .retry-delivery:disabled { opacity: 0.5; cursor: not-allowed; }
  .retry-delivery svg { width: 1rem; height: 1rem; fill: none; stroke: currentColor; stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; }
  .chat-bubble :global(img) { max-width: 100%; height: auto; }
  .chat-bubble :global(pre) { max-width: 100%; box-sizing: border-box; }
  .chat-bubble :global(table) { display: block; max-width: 100%; overflow-x: auto; }
  @media (max-width: 600px) { .chat-bubble { max-width: 92%; padding: var(--space-2) var(--space-3); } }
</style>
