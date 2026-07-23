<script lang="ts">
  import type { Snippet } from 'svelte';
  import './sidebar.css';

  type Props = {
    children: Snippet<[]>;
  };

  let { children }: Props = $props();
  let folded = $state(false);

  function toggleFold() {
    folded = !folded;
  }
</script>

<aside class="sidebar-frame" class:folded aria-label="Sidebar">
  <div class="sidebar-control-row">
    <button
      class="sidebar-fold-button"
      type="button"
      aria-label={folded ? 'Unfold sidebar' : 'Fold sidebar'}
      aria-expanded={!folded}
      title={folded ? 'Unfold sidebar' : 'Fold sidebar'}
      onclick={toggleFold}
    >
      {#if folded}
        <svg class="sidebar-icon" aria-hidden="true" viewBox="0 0 24 24">
          <path d="m6 17 5-5-5-5" />
          <path d="m13 17 5-5-5-5" />
        </svg>
      {:else}
        <svg class="sidebar-icon" aria-hidden="true" viewBox="0 0 24 24">
          <path d="m11 17-5-5 5-5" />
          <path d="m18 17-5-5 5-5" />
        </svg>
      {/if}
    </button>
  </div>

  {#if !folded}
    <div class="sidebar-frame-content">
      {@render children()}
    </div>
  {/if}
</aside>
