<script lang="ts">
  import type { ConsoleDisplayState } from "./console-display-state";

  type Props = {
    state: ConsoleDisplayState;
    hasContent: boolean;
    hasUnfilteredContent: boolean;
    onRetry: () => void;
  };

  let { state, hasContent, hasUnfilteredContent, onRetry }: Props = $props();
</script>

{#if state.kind === "loading"}
  <div class="console-display-state loading" role="status" aria-live="polite">
    <span class="loading-spinner" aria-hidden="true"></span>
    <span>
      <strong>Loading conversation</strong>
      {#if state.stage === "session"}
        <small>Checking Worker Session availability…</small>
      {:else}
        <small>Waiting for the initial conversation snapshot…</small>
      {/if}
    </span>
  </div>
{:else if state.kind === "unavailable" || state.kind === "failed"}
  <div class="console-display-state unavailable" role="alert">
    <span>
      <strong>
        {state.kind === "unavailable"
          ? "Conversation unavailable"
          : "Unable to load conversation"}
      </strong>
      <small>{state.reason}</small>
    </span>
    <button type="button" onclick={onRetry}>Retry</button>
  </div>
{:else}
  {#if state.kind === "stale"}
    <div class="console-display-state stale" role="status" aria-live="polite">
      <span>
        <strong>
          {state.phase === "reconnecting"
            ? "Refreshing conversation"
            : "Conversation updates are unavailable"}
        </strong>
        <small>{state.reason} Previously loaded conversation is preserved.</small>
      </span>
      <button type="button" onclick={onRetry}>Retry</button>
    </div>
  {:else if state.source === "retained"}
    <div class="console-display-state retained" role="status">
      <span>
        <strong>Read-only retained conversation</strong>
        <small>This saved Session snapshot cannot receive live updates.</small>
      </span>
    </div>
  {/if}

  {#if state.kind === "ready" && !hasContent}
    <div class="console-display-state empty" role="status">
      <span>
        <strong>No conversation to display</strong>
        <small>
          {hasUnfilteredContent
            ? "No console items match the current display mode."
            : "This Worker view has no conversation history yet."}
        </small>
      </span>
    </div>
  {/if}
{/if}

<style>
  .console-display-state {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    border: 1px solid var(--line);
    border-radius: 12px;
    padding: var(--space-4);
    color: var(--text-muted);
    background: color-mix(in srgb, var(--bg-raised) 78%, transparent);
  }

  .console-display-state > span {
    display: grid;
    gap: var(--space-1);
  }

  .console-display-state strong {
    color: var(--text-strong);
  }

  .console-display-state small {
    font-size: var(--font-size-compact);
    line-height: var(--line-height-compact);
  }

  .console-display-state button {
    flex: 0 0 auto;
    border: 1px solid var(--line);
    border-radius: 999px;
    padding: 0.4rem 0.75rem;
    background: var(--bg);
    color: var(--text-strong);
    cursor: pointer;
    font: inherit;
    font-size: var(--font-size-compact);
    font-weight: 700;
  }

  .console-display-state.loading {
    justify-content: flex-start;
  }

  .console-display-state.unavailable {
    border-color: color-mix(in srgb, var(--danger) 55%, var(--line));
  }

  .console-display-state.stale {
    border-color: color-mix(in srgb, var(--tui-yellow) 48%, var(--line));
  }

  .console-display-state.retained {
    border-color: color-mix(in srgb, var(--tui-cyan) 42%, var(--line));
  }

  .loading-spinner {
    width: 1rem;
    height: 1rem;
    flex: 0 0 auto;
    border: 2px solid color-mix(in srgb, var(--accent) 30%, var(--line));
    border-top-color: var(--accent);
    border-radius: 999px;
  }

  @media (prefers-reduced-motion: no-preference) {
    .loading-spinner {
      animation: console-loading-spin 800ms linear infinite;
    }
  }

  @keyframes console-loading-spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
