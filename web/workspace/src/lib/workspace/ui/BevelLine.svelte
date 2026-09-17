<script lang="ts">
  export type BevelLineDirection = 'x' | 'y';

  type Props = {
    direction?: BevelLineDirection;
    length?: string;
    decorative?: boolean;
    as?: 'span' | 'div';
    class?: string;
  };

  let {
    direction = 'x',
    length = '100%',
    decorative = false,
    as = 'span',
    class: className = '',
  }: Props = $props();
</script>

<svelte:element
  this={as}
  class={`bevel-line ${className}`.trim()}
  data-direction={direction}
  role={decorative ? undefined : 'separator'}
  aria-orientation={decorative ? undefined : direction === 'x' ? 'horizontal' : 'vertical'}
  aria-hidden={decorative ? 'true' : undefined}
  style:width={direction === 'x' ? length : undefined}
  style:height={direction === 'y' ? length : undefined}
></svelte:element>

<style>
  .bevel-line {
    display: block;
    flex: 0 0 auto;
    border-color: var(--line-strong);
    pointer-events: none;
  }

  .bevel-line[data-direction='x'] {
    height: 0;
    border-top-style: solid;
    border-top-width: 1px;
  }

  .bevel-line[data-direction='y'] {
    width: 0;
    border-left-style: solid;
    border-left-width: 1px;
  }

  @media (forced-colors: active) {
    .bevel-line {
      border-color: CanvasText;
      forced-color-adjust: auto;
    }
  }

  @media print {
    .bevel-line {
      border-color: currentColor;
    }
  }
</style>
