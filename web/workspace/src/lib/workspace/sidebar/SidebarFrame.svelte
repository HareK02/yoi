<script lang="ts">
  import type { Snippet } from 'svelte';
  import Bevel from '$lib/workspace/ui/Bevel.svelte';
  import './sidebar.css';

  type Props = {
    children: Snippet<[]>;
    folded?: boolean;
  };

  let { children, folded = $bindable(false) }: Props = $props();

  function toggleFold() {
    folded = !folded;
  }
</script>

<div class="sidebar-frame" class:folded>
  <Bevel as="div" class="sidebar-frame__bevel" depth="inset" top={false} bottom={false} left={false} fill>
    <aside class="sidebar-frame__surface" aria-label="Sidebar">
      {#if !folded}
        <div class="sidebar-frame-content">
          {@render children()}
        </div>
      {/if}

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
    </aside>
  </Bevel>
</div>
