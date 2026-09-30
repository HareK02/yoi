<script lang="ts">
  import type { ConsoleTurn } from './turn-navigation';

  let { turns, onSelect }: {
    turns: ConsoleTurn[];
    onSelect: (id: string) => void;
  } = $props();

  const tooltipId = $props.id();
  let hoveredId = $state<string | null>(null);
  let focusedId = $state<string | null>(null);
  let dismissed = $state(false);
  let anchorY = $state(0);
  let railHeight = $state(0);
  let previewHeight = $state(0);
  let rail: HTMLElement;
  const preview = $derived(dismissed ? undefined : turns.find(
    (turn) => turn.id === (hoveredId ?? focusedId),
  ));
  const previewTop = $derived(Math.max(0, Math.min(anchorY, railHeight - previewHeight)));

  function show(event: MouseEvent | FocusEvent, id: string) {
    const button = event.currentTarget as HTMLElement;
    anchorY = button.getBoundingClientRect().top - rail.getBoundingClientRect().top;
    dismissed = false;
    if (event.type === 'focus') focusedId = id;
    else hoveredId = id;
  }
</script>

<svelte:window onkeydown={(event) => { if (event.key === 'Escape') dismissed = true; }} />

<nav
  class="turn-navigation"
  aria-label="Conversation turns"
  bind:this={rail}
  bind:clientHeight={railHeight}
  onmouseleave={() => { hoveredId = null; }}
>
  <div class="turn-bars" onscroll={() => { dismissed = true; }}>
    {#each turns as turn, index (turn.id)}
      <button
        type="button"
        class="turn-button"
        class:previewing={preview?.id === turn.id}
        aria-label={`Turn ${index + 1}: ${turn.user || 'User message'}`}
        aria-describedby={preview?.id === turn.id ? tooltipId : undefined}
        onmouseenter={(event) => show(event, turn.id)}
        onfocus={(event) => show(event, turn.id)}
        onblur={() => { focusedId = null; }}
        onclick={() => onSelect(turn.id)}
      ><span class="turn-bar" aria-hidden="true"></span></button>
    {/each}
  </div>
  {#if preview}
    <div
      class="turn-preview-position"
      style:top={`${previewTop}px`}
      bind:clientHeight={previewHeight}
    >
      <div class="turn-preview" id={tooltipId} role="tooltip">
        <p class="preview-user">{preview.user || '—'}</p>
        <p class="preview-assistant">{preview.assistant || 'No response yet'}</p>
      </div>
    </div>
  {/if}
</nav>

<style>
  .turn-navigation {
    position: relative;
    min-height: 0;
    min-width: 0;
  }

  .turn-bars {
    display: flex;
    flex-direction: column;
    align-items: center;
    height: 100%;
    overflow-y: auto;
    scrollbar-width: none;
    overscroll-behavior: contain;
  }

  .turn-bars::-webkit-scrollbar { display: none; }

  /* Center short lists without making overflowing turns unreachable. */
  .turn-button:first-child { margin-top: auto; }
  .turn-button:last-child { margin-bottom: auto; }

  .turn-button {
    display: grid;
    place-items: center;
    flex: 0 0 24px;
    width: 32px;
    padding: 0;
    border: 0;
    border-radius: 4px;
    background: transparent;
    cursor: pointer;
  }

  .turn-bar {
    width: 18px;
    height: 3px;
    border-radius: 999px;
    background: var(--text-muted);
    opacity: 0.5;
  }

  .turn-button.previewing .turn-bar,
  .turn-button:hover .turn-bar,
  .turn-button:focus-visible .turn-bar {
    width: 26px;
    background: var(--tui-cyan);
    opacity: 1;
  }

  .turn-button:focus-visible {
    outline: 1px solid var(--tui-cyan);
    outline-offset: -1px;
  }

  .turn-preview-position {
    position: absolute;
    z-index: 20;
    right: 100%;
    width: min(24rem, calc(100vw - 5rem));
    max-height: 100%;
    overflow: hidden;
    padding-right: var(--space-2);
    box-sizing: border-box;
  }

  .turn-preview {
    display: grid;
    gap: var(--space-2);
    padding: var(--space-3);
    border: 1px solid var(--line);
    border-radius: 8px;
    background: var(--bg-raised);
    color: var(--text-strong);
    box-shadow: var(--shadow-overlay);
    font-size: var(--font-size-body);
    line-height: var(--line-height-body);
  }

  .turn-preview p { margin: 0; min-width: 0; line-height: inherit; }

  .preview-user {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .preview-assistant {
    color: var(--text-muted);
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    overflow: hidden;
    overflow-wrap: anywhere;
  }
</style>
