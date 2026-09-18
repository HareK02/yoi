<script lang="ts">
  import { onDestroy, onMount, tick, type Snippet } from 'svelte';

  type Placement = 'top' | 'bottom';
  type Alignment = 'start' | 'center' | 'end';

  let {
    id,
    text,
    placement = 'top',
    align = 'center',
    children,
  }: {
    id: string;
    text: string;
    placement?: Placement;
    align?: Alignment;
    children: Snippet<[descriptionId: string]>;
  } = $props();

  let open = $state(false);
  let pointerInside = false;
  let focusInside = false;
  let hoverTimer: ReturnType<typeof globalThis.setTimeout> | undefined;
  let rootElement: HTMLSpanElement | undefined;
  let tooltipElement: HTMLSpanElement | undefined;
  let tooltipLeft = $state(0);
  let tooltipTop = $state(0);

  function clearHoverTimer() {
    if (hoverTimer === undefined) return;
    globalThis.clearTimeout(hoverTimer);
    hoverTimer = undefined;
  }

  function positionTooltip() {
    if (!open || rootElement === undefined || tooltipElement === undefined) return;

    const root = rootElement.getBoundingClientRect();
    const tooltip = tooltipElement.getBoundingClientRect();
    const margin = 8;
    const gap = 8;

    let left = root.left + (root.width - tooltip.width) / 2;
    if (align === 'start') left = root.left;
    if (align === 'end') left = root.right - tooltip.width;
    left = Math.max(margin, Math.min(left, globalThis.innerWidth - tooltip.width - margin));

    let top = placement === 'top' ? root.top - tooltip.height - gap : root.bottom + gap;
    if (top < margin) top = root.bottom + gap;
    if (top + tooltip.height > globalThis.innerHeight - margin) {
      top = root.top - tooltip.height - gap;
    }
    top = Math.max(margin, Math.min(top, globalThis.innerHeight - tooltip.height - margin));

    tooltipLeft = Math.round(left);
    tooltipTop = Math.round(top);
  }

  function reveal() {
    open = true;
    void tick().then(positionTooltip);
  }

  function showAfterDelay() {
    pointerInside = true;
    clearHoverTimer();
    hoverTimer = globalThis.setTimeout(() => {
      reveal();
      hoverTimer = undefined;
    }, 300);
  }

  function showImmediately() {
    focusInside = true;
    clearHoverTimer();
    reveal();
  }

  function dismiss() {
    clearHoverTimer();
    open = false;
  }

  function handlePointerLeave() {
    pointerInside = false;
    clearHoverTimer();
    if (!focusInside) open = false;
  }

  function handleFocusOut(event: FocusEvent) {
    const root = event.currentTarget;
    if (root instanceof HTMLElement && event.relatedTarget instanceof Node && root.contains(event.relatedTarget)) {
      return;
    }
    focusInside = false;
    if (!pointerInside) open = false;
  }

  function handleWindowKeydown(event: KeyboardEvent) {
    if (open && event.key === 'Escape') dismiss();
  }

  onMount(() => {
    const reposition = () => positionTooltip();
    globalThis.addEventListener('resize', reposition);
    document.addEventListener('scroll', reposition, true);
    return () => {
      globalThis.removeEventListener('resize', reposition);
      document.removeEventListener('scroll', reposition, true);
    };
  });

  onDestroy(clearHoverTimer);
</script>

<svelte:window onkeydown={handleWindowKeydown} />

<span
  class="tooltip-root"
  bind:this={rootElement}
  onpointerenter={showAfterDelay}
  onpointerleave={handlePointerLeave}
  onfocusin={showImmediately}
  onfocusout={handleFocusOut}
>
  {@render children(id)}
  <span
    {id}
    class:open
    class="tooltip-content"
    role="tooltip"
    bind:this={tooltipElement}
    style:left={`${tooltipLeft}px`}
    style:top={`${tooltipTop}px`}
  >
    {text}
  </span>
</span>

<style>
  .tooltip-root {
    display: inline-flex;
    width: fit-content;
  }

  .tooltip-content {
    position: fixed;
    z-index: 50;
    width: max-content;
    max-width: min(20rem, calc(100vw - var(--space-4)));
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-soft);
    padding: var(--space-2) var(--space-3);
    background: var(--text-strong);
    box-shadow: var(--shadow-overlay);
    color: var(--bg);
    font-family: var(--font-sans);
    font-size: var(--font-size-compact);
    font-weight: 500;
    line-height: 16px;
    opacity: 0;
    pointer-events: none;
    text-align: left;
    visibility: hidden;
  }

  .tooltip-content.open {
    opacity: 1;
    visibility: visible;
  }

  @media (prefers-reduced-motion: no-preference) {
    .tooltip-content {
      transition: opacity 100ms ease;
    }
  }
</style>
