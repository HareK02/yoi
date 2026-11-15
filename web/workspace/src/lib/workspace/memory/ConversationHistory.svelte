<script lang="ts">
  import { untrack } from 'svelte';
  import type { ChatMessage } from '#lib/generated/chat-api.ts';
  import { followTranscriptTail, isNearTranscriptBottom, observeTranscriptSize } from '#lib/workspace/transcript/scroll.ts';
  import ConversationBubble from './ConversationBubble.svelte';
  import { conversationMessageView } from './conversation-view.ts';

  let { messages, accountId, subjectName }: { messages: ChatMessage[]; accountId: string; subjectName: string } = $props();
  let scroller = $state<HTMLElement | null>(null);
  let content = $state<HTMLElement | null>(null);
  let following = $state(true);
  const rows = $derived(messages.map(message => conversationMessageView(message, accountId, subjectName)));
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
    <ol bind:this={content} aria-label="Saved public messages">
      {#each rows as message (message.id)}<ConversationBubble {message} />{/each}
    </ol>
    {#if messages.length === 0}<p class="empty-history">No public messages yet.</p>{/if}
  </div>
  {#if !following}<button class="workspace-secondary-button latest" type="button" onclick={jump}>Jump to latest</button>{/if}
</div>

<style>
  .history-region { position: relative; min-width: 0; }
  .chat-scroll { overflow-y: auto; overscroll-behavior: contain; min-height: 10rem; max-height: min(45dvh, 36rem); padding: var(--space-2); scrollbar-gutter: stable; }
  .chat-scroll:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
  ol { list-style: none; margin: 0; padding: 0; display: grid; gap: var(--space-4); }
  .empty-history { color: var(--text-muted); margin: var(--space-4) 0; }
  .latest { display: block; margin: var(--space-2) auto 0; }
</style>
