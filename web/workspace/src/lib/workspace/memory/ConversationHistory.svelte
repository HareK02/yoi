<script lang="ts">
  import { untrack } from 'svelte';
  import type { ChatMessage } from '#lib/generated/chat-api.ts';
  import { followTranscriptTail, isNearTranscriptBottom, observeTranscriptSize } from '#lib/workspace/transcript/scroll.ts';
  import JumpToLatest from '#lib/workspace/transcript/JumpToLatest.svelte';
  import ConversationBubble from './ConversationBubble.svelte';
  import { conversationHistoryView, type ConversationOutgoing } from './conversation-view.ts';

  let { messages, accountId, subjectName, outgoing = [], onretry, retryDisabled = false }: {
    messages: ChatMessage[];
    accountId: string;
    subjectName: string;
    outgoing?: ConversationOutgoing[];
    onretry?: (operationId: string) => void;
    retryDisabled?: boolean;
  } = $props();
  let scroller = $state<HTMLElement | null>(null);
  let content = $state<HTMLElement | null>(null);
  let following = $state(true);
  const rows = $derived(conversationHistoryView(messages, accountId, subjectName, outgoing));
  function follow() {
    const element = scroller;
    void followTranscriptTail(element, () => following && scroller === element);
  }
  function jump() { following = true; follow(); }
  $effect(() => { rows; untrack(follow); });
  $effect(() => {
    const element = content;
    if (element) return observeTranscriptSize(element, follow, scroller);
  });
</script>

<div class="history-region">
  <!-- svelte-ignore a11y_no_noninteractive_tabindex (履歴の独立スクロール領域をキーボードで操作するため) -->
  <div class="chat-scroll" bind:this={scroller} onscroll={() => { if (scroller) following = isNearTranscriptBottom(scroller); }} role="region" tabindex="0" aria-label="Conversation history">
    <ol bind:this={content} aria-label="Conversation messages">
      {#each rows as message (`${message.local ? 'local' : 'saved'}:${message.id}`)}<ConversationBubble {message} {onretry} {retryDisabled} />{/each}
    </ol>
    {#if rows.length === 0}<p class="empty-history">No public messages yet.</p>{/if}
  </div>
  {#if !following && rows.length > 0}<JumpToLatest onclick={jump} />{/if}
</div>

<style>
  .history-region { display: grid; grid-template: minmax(0, 1fr) / minmax(0, 1fr); flex: 1 1 0; min-width: 0; min-height: 0; }
  .chat-scroll { grid-area: 1 / 1; min-height: 0; overflow-y: auto; overscroll-behavior: contain; padding: var(--space-2); scrollbar-gutter: stable; }
  .chat-scroll:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
  ol { list-style: none; margin: 0; padding: 0; display: grid; gap: var(--space-4); }
  .empty-history { color: var(--text-muted); margin: var(--space-4) 0; }
</style>
