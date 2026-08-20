<script lang="ts">
  type Props = {
    values: readonly string[];
    intervalMs?: number;
    ariaLabel?: string;
    class?: string;
  };

  let {
    values,
    intervalMs = 100,
    ariaLabel,
    class: className,
  }: Props = $props();
  let index = $state(0);
  const value = $derived(values.length > 0 ? values[index % values.length] : "");

  $effect(() => {
    const length = values.length;
    const delay = Math.max(16, intervalMs);
    index = 0;
    if (length <= 1) return;

    const timer = window.setInterval(() => {
      index = (index + 1) % length;
    }, delay);
    return () => window.clearInterval(timer);
  });
</script>

<span
  class={className}
  class:sequence-loop={true}
  aria-label={ariaLabel}
  aria-hidden={ariaLabel ? undefined : "true"}
>{value}</span>

<style>
  .sequence-loop {
    display: inline-block;
    min-width: 1ch;
    text-align: center;
    font-variant-numeric: tabular-nums;
  }
</style>
