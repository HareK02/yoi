<script lang="ts">
  export type BevelLineDirection = 'x' | 'y';
  export type BevelLineDepth = 'raised' | 'inset';

  type Props = {
    direction?: BevelLineDirection;
    length?: string;
    depth?: BevelLineDepth;
    decorative?: boolean;
    as?: 'span' | 'div';
    class?: string;
  };

  let {
    direction = 'x',
    length = '100%',
    depth = 'raised',
    decorative = false,
    as = 'span',
    class: className = '',
  }: Props = $props();
</script>

<svelte:element
  this={as}
  class={`bevel-line ${className}`.trim()}
  data-direction={direction}
  data-depth={depth}
  role={decorative ? undefined : 'separator'}
  aria-orientation={decorative ? undefined : direction === 'x' ? 'horizontal' : 'vertical'}
  aria-hidden={decorative ? 'true' : undefined}
  style:width={direction === 'x' ? length : undefined}
  style:height={direction === 'y' ? length : undefined}
></svelte:element>

<style>
  .bevel-line {
    --bevel-light: 315deg;
    --bevel-line-face-width: var(--bevel-face-width);
    --bevel-line-width: calc(var(--bevel-line-face-width) * 2);

    display: block;
    flex: 0 0 auto;
    overflow: hidden;
    border-radius: 999px;
    background: var(--line-strong);
    pointer-events: none;
  }

  .bevel-line[data-direction='x'] {
    height: var(--bevel-line-width);
  }

  .bevel-line[data-direction='y'] {
    width: var(--bevel-line-width);
  }

  @supports
    (color: color-mix(in oklab, white 50%, black)) and
    (width: calc(cos(0deg) * 1px)) {
    .bevel-line {
      --_L: var(--bevel-light);
      --_s0: var(--bevel-highlight);
      --_s1: color-mix(in oklab, var(--bevel-highlight) 96.5926%, var(--bevel-shadow));
      --_s2: color-mix(in oklab, var(--bevel-highlight) 86.6025%, var(--bevel-shadow));
      --_s3: color-mix(in oklab, var(--bevel-highlight) 70.7107%, var(--bevel-shadow));
      --_s4: color-mix(in oklab, var(--bevel-highlight) 50%, var(--bevel-shadow));
      --_s5: color-mix(in oklab, var(--bevel-highlight) 25.8819%, var(--bevel-shadow));
      --_s6: var(--bevel-shadow);
      --_shade:
        var(--_s0) 0deg,
        var(--_s1) 15deg,
        var(--_s2) 30deg,
        var(--_s3) 45deg,
        var(--_s4) 60deg,
        var(--_s5) 75deg,
        var(--_s6) 90deg,
        var(--_s6) 270deg,
        var(--_s5) 285deg,
        var(--_s4) 300deg,
        var(--_s3) 315deg,
        var(--_s2) 330deg,
        var(--_s1) 345deg,
        var(--_s0) 360deg;
      --_top: color-mix(
        in oklab,
        var(--bevel-highlight) calc(max(cos(0deg - var(--_L)), 0) * 100%),
        var(--bevel-shadow)
      );
      --_right: color-mix(
        in oklab,
        var(--bevel-highlight) calc(max(cos(90deg - var(--_L)), 0) * 100%),
        var(--bevel-shadow)
      );
      --_bottom: color-mix(
        in oklab,
        var(--bevel-highlight) calc(max(cos(180deg - var(--_L)), 0) * 100%),
        var(--bevel-shadow)
      );
      --_left: color-mix(
        in oklab,
        var(--bevel-highlight) calc(max(cos(270deg - var(--_L)), 0) * 100%),
        var(--bevel-shadow)
      );
    }

    .bevel-line[data-depth='inset'] {
      --_L: calc(var(--bevel-light) + 180deg);
    }

    .bevel-line[data-direction='x'] {
      background:
        conic-gradient(from var(--_L) at 100% 50%, var(--_shade)) 0 0 /
          var(--bevel-line-face-width) var(--bevel-line-width) no-repeat,
        conic-gradient(from var(--_L) at 0 50%, var(--_shade)) 100% 0 /
          var(--bevel-line-face-width) var(--bevel-line-width) no-repeat,
        linear-gradient(to bottom, var(--_top) 0 50%, var(--_bottom) 50% 100%)
          var(--bevel-line-face-width) 0 /
          calc(100% - var(--bevel-line-width)) 100% no-repeat;
    }

    .bevel-line[data-direction='y'] {
      background:
        conic-gradient(from var(--_L) at 50% 100%, var(--_shade)) 0 0 /
          var(--bevel-line-width) var(--bevel-line-face-width) no-repeat,
        conic-gradient(from var(--_L) at 50% 0, var(--_shade)) 0 100% /
          var(--bevel-line-width) var(--bevel-line-face-width) no-repeat,
        linear-gradient(to right, var(--_left) 0 50%, var(--_right) 50% 100%)
          0 var(--bevel-line-face-width) / 100%
          calc(100% - var(--bevel-line-width)) no-repeat;
    }
  }

  @media (forced-colors: active) {
    .bevel-line {
      background: CanvasText;
      forced-color-adjust: auto;
    }
  }

  @media print {
    .bevel-line {
      background: currentColor;
    }
  }
</style>
