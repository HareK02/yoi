<script lang="ts">
  import RichMarkdown from '#lib/workspace/console/RichMarkdown.svelte';
  import type { ConversationMessageView } from './conversation-view.ts';
  let { message }: { message: ConversationMessageView } = $props();
</script>

<li class="chat-message" class:own={message.own} data-message-id={message.id}>
  <div class="chat-bubble">
    <div class="chat-author">{message.author}</div>
    <RichMarkdown text={message.text} streamId={message.id} />
  </div>
</li>

<style>
  .chat-message { display: flex; justify-content: flex-start; min-width: 0; }
  .chat-message.own { justify-content: flex-end; }
  .chat-bubble { min-width: 0; max-width: min(85%, 48rem); padding: var(--space-3) var(--space-4); background: var(--bg-subtle); border-radius: var(--space-3); overflow-wrap: anywhere; }
  .own .chat-bubble { background: var(--bg-raised); }
  .chat-author { font-size: var(--font-size-compact); line-height: var(--line-height-compact); color: var(--text-muted); margin-bottom: var(--space-1); }
  .chat-bubble :global(img) { max-width: 100%; height: auto; }
  .chat-bubble :global(pre) { max-width: 100%; box-sizing: border-box; }
  .chat-bubble :global(table) { display: block; max-width: 100%; overflow-x: auto; }
  @media (max-width: 600px) { .chat-bubble { max-width: 92%; padding: var(--space-2) var(--space-3); } }
</style>
