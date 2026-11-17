<script lang="ts">
  import { prefersReducedMotion } from "svelte/motion";
  import { fly } from "svelte/transition";
  let { onclick }: { onclick: () => void } = $props();
</script>

<button class="console-jump-latest" type="button" aria-label="Jump to latest" title="Jump to latest"
  transition:fly={{ y: 6, duration: prefersReducedMotion.current ? 0 : 160 }} {onclick}>
  <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 4V16M7 11L12 16L17 11M5 20H19" /></svg>
</button>

<style>
    .console-jump-latest {
        grid-area: 1 / 1;
        align-self: end;
        justify-self: center;
        z-index: 2;
        display: grid;
        place-items: center;
        width: 36px;
        height: 36px;
        padding: 0;
        margin-bottom: var(--space-3);
        border: 1px solid var(--line);
        border-radius: 50%;
        background: var(--bg-raised);
        color: var(--text-strong);
        box-shadow: var(--shadow-overlay);
        cursor: pointer;
    }

    .console-jump-latest:hover {
        color: var(--tui-cyan);
        border-color: var(--tui-cyan);
    }

    @media (prefers-reduced-motion: no-preference) {
        .console-jump-latest {
            transition: translate 140ms ease-out, color 140ms ease-out,
                border-color 140ms ease-out;
        }

        .console-jump-latest:hover {
            translate: 0 -2px;
        }
    }

    .console-jump-latest:focus-visible {
        outline: 2px solid var(--tui-cyan);
        outline-offset: 2px;
    }

    .console-jump-latest svg {
        width: 20px;
        height: 20px;
        fill: none;
        stroke: currentColor;
        stroke-width: 1.75;
        stroke-linecap: round;
        stroke-linejoin: round;
    }

</style>
