<script lang="ts">
  import type { TurnNavigationItem } from "./turn-navigation";

  type Props = {
    items: TurnNavigationItem[];
    hasMore: boolean;
    loading: boolean;
    error: string | null;
    onLoadMore: () => void;
    onTurnClick: (item: TurnNavigationItem) => void;
    onTopEdgeChange: (atTop: boolean) => void;
    historyAvailable?: boolean;
    element?: HTMLElement | null;
  };

  let {
    items,
    hasMore,
    loading,
    error,
    onLoadMore,
    onTurnClick,
    onTopEdgeChange,
    historyAvailable = true,
    element = $bindable(null),
  }: Props = $props();

  function handleScroll(event: Event) {
    const target = event.currentTarget;
    if (!(target instanceof HTMLElement)) return;
    onTopEdgeChange(target.scrollTop <= 1);
  }
</script>

<aside class="turn-navigation" aria-label="Conversation turns">
  <div class="turn-list" bind:this={element} onscroll={handleScroll}>
    <div class="history-boundary" class:complete={!hasMore && !error && !loading}>
      {#if !historyAvailable}
        <span>Earlier history is unavailable for this view</span>
      {:else if loading}
        <button type="button" disabled>Loading earlier conversation…</button>
      {:else if error}
        <button type="button" onclick={onLoadMore}>Retry earlier conversation</button>
        <span title={error}>History unavailable</span>
      {:else if hasMore}
        <button type="button" onclick={onLoadMore}>Earlier conversation available</button>
      {:else}
        <span>Start of conversation</span>
      {/if}
    </div>

    {#each items as item (item.turnId)}
      <button
        type="button"
        class="turn"
        data-turn-id={item.turnId}
        aria-label={`Jump to conversation: ${item.user}`}
        onclick={() => onTurnClick(item)}
      >
        <span class="bar" aria-hidden="true"></span>
        <span class="preview">
          <span class="user-preview">{item.user}</span>
          {#each item.assistant as line}
            <span class="assistant-preview">{line}</span>
          {/each}
        </span>
      </button>
    {/each}
  </div>
</aside>

<style>
  .turn-navigation {
    min-width: 0;
    min-height: 0;
    height: 100%;
    border-left: 1px solid var(--line);
    background: color-mix(in srgb, var(--bg-raised) 82%, transparent);
  }

  .turn-list {
    height: 100%;
    overflow-y: auto;
    overscroll-behavior: contain;
    padding: var(--space-2) var(--space-2) var(--space-4);
  }

  .history-boundary {
    display: grid;
    gap: 0.2rem;
    justify-items: center;
    min-height: 2.25rem;
    color: var(--text-muted);
    font-size: var(--font-size-compact);
    text-align: center;
  }

  .history-boundary button {
    border: 0;
    background: transparent;
    color: var(--tui-cyan);
    cursor: pointer;
    font: inherit;
    padding: 0.25rem;
  }

  .history-boundary button:disabled {
    cursor: wait;
    opacity: 0.65;
  }

  .history-boundary.complete::before {
    content: "";
    width: 2rem;
    border-top: 1px solid var(--line);
  }

  .turn {
    display: grid;
    width: 100%;
    grid-template-columns: 1.6rem minmax(0, 1fr);
    align-items: center;
    border: 0;
    background: transparent;
    color: inherit;
    cursor: pointer;
    padding: 0.42rem 0;
    text-align: left;
  }

  .bar {
    width: 1.1rem;
    border-top: 3px solid var(--tui-blue);
    border-radius: 999px;
    justify-self: center;
  }

  .preview {
    display: grid;
    min-width: 0;
    gap: 0.12rem;
  }

  .user-preview,
  .assistant-preview {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .user-preview {
    color: var(--text);
    font-size: var(--font-size-compact);
  }

  .assistant-preview {
    color: var(--text-muted);
    font-size: var(--font-size-compact);
  }

  .turn:focus-visible,
  .turn:hover {
    background: color-mix(in srgb, var(--tui-blue) 9%, transparent);
    outline: none;
  }
</style>
