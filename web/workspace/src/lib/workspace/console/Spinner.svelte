<script module lang="ts">
  export const SPINNER_FRAMES = [
    "⣷",
    "⣯",
    "⣟",
    "⡿",
    "⢿",
    "⣻",
    "⣽",
    "⣾",
  ] as const;
</script>

<script lang="ts">
  import SequenceLoop from "./SequenceLoop.svelte";

  type Props = {
    intervalMs?: number;
    label?: string;
  };

  let { intervalMs = 90, label = "Running" }: Props = $props();
</script>

<span class="spinner" role="img" aria-label={label}>
  <SequenceLoop values={SPINNER_FRAMES} {intervalMs} />
</span>

<style>
  .spinner {
    display: inline-flex;
    color: var(--spinner-color, var(--accent));
    line-height: 1;
  }
</style>
