<script lang="ts">
  import type { Snippet } from 'svelte';

  export type BevelProfile = 'edge' | 'ridge';
  export type BevelDepth = 'raised' | 'inset';

  type Props = {
    children: Snippet<[]>;
    profile?: BevelProfile;
    depth?: BevelDepth;
    top?: boolean;
    right?: boolean;
    bottom?: boolean;
    left?: boolean;
    fill?: boolean;
    invalid?: boolean;
    pressed?: boolean;
    as?: 'span' | 'div';
    class?: string;
  };

  let {
    children,
    profile = 'edge',
    depth = 'raised',
    top = true,
    right = true,
    bottom = true,
    left = true,
    fill = false,
    invalid = false,
    pressed = false,
    as = 'span',
    class: className = '',
  }: Props = $props();
</script>

<svelte:element
  this={as}
  class={`bevel ${fill ? 'bevel--fill' : ''} ${className}`.trim()}
  data-profile={profile}
  data-depth={depth}
  data-top={top ? 'true' : 'false'}
  data-right={right ? 'true' : 'false'}
  data-bottom={bottom ? 'true' : 'false'}
  data-left={left ? 'true' : 'false'}
  data-invalid={invalid || undefined}
  data-pressed={pressed || undefined}
>
  {@render children()}
</svelte:element>

<style>
  .bevel {
    --bevel-light: 315deg;
    --bevel-radius: var(--radius-soft);
    --bevel-top-left-radius: var(--bevel-radius);
    --bevel-top-right-radius: var(--bevel-radius);
    --bevel-bottom-right-radius: var(--bevel-radius);
    --bevel-bottom-left-radius: var(--bevel-radius);
    --bevel-top-inset: var(--bevel-face-width);
    --bevel-right-inset: var(--bevel-face-width);
    --bevel-bottom-inset: var(--bevel-face-width);
    --bevel-left-inset: var(--bevel-face-width);

    position: relative;
    isolation: isolate;
    display: inline-grid;
    min-width: 0;
    border-radius:
      var(--bevel-top-left-radius)
      var(--bevel-top-right-radius)
      var(--bevel-bottom-right-radius)
      var(--bevel-bottom-left-radius);
    vertical-align: middle;
  }

  .bevel[data-top='false'] {
    --bevel-top-left-radius: 0px;
    --bevel-top-right-radius: 0px;
    --bevel-top-inset: 0px;
  }

  .bevel[data-right='false'] {
    --bevel-top-right-radius: 0px;
    --bevel-bottom-right-radius: 0px;
    --bevel-right-inset: 0px;
  }

  .bevel[data-bottom='false'] {
    --bevel-bottom-right-radius: 0px;
    --bevel-bottom-left-radius: 0px;
    --bevel-bottom-inset: 0px;
  }

  .bevel[data-left='false'] {
    --bevel-bottom-left-radius: 0px;
    --bevel-top-left-radius: 0px;
    --bevel-left-inset: 0px;
  }

  .bevel--fill {
    width: 100%;
  }

  .bevel > :global(*) {
    position: relative;
    z-index: 1;
    min-width: 0;
    border-radius: inherit;
  }

  .bevel > :global(:is(button, input, select, textarea)) {
    border-color: transparent;
  }

  .bevel::before,
  .bevel[data-profile='ridge']::after {
    --_W: var(--bevel-face-width);
    --_top-w: var(--_W);
    --_right-w: var(--_W);
    --_bottom-w: var(--_W);
    --_left-w: var(--_W);

    content: '';
    position: absolute;
    z-index: 2;
    inset: 0;
    box-sizing: border-box;
    border-style: solid;
    border-color: var(--line-strong);
    border-width: var(--_top-w) var(--_right-w) var(--_bottom-w) var(--_left-w);
    border-radius: inherit;
    pointer-events: none;
  }

  .bevel[data-top='false']::before,
  .bevel[data-top='false'][data-profile='ridge']::after {
    --_top-w: 0px;
  }

  .bevel[data-right='false']::before,
  .bevel[data-right='false'][data-profile='ridge']::after {
    --_right-w: 0px;
  }

  .bevel[data-bottom='false']::before,
  .bevel[data-bottom='false'][data-profile='ridge']::after {
    --_bottom-w: 0px;
  }

  .bevel[data-left='false']::before,
  .bevel[data-left='false'][data-profile='ridge']::after {
    --_left-w: 0px;
  }

  .bevel[data-profile='edge'][data-depth='inset']::before {
    border-color: var(--line);
  }

  .bevel[data-profile='ridge'][data-depth='inset']::before {
    border-color: var(--line);
  }

  .bevel[data-profile='ridge']::after {
    inset:
      var(--bevel-top-inset)
      var(--bevel-right-inset)
      var(--bevel-bottom-inset)
      var(--bevel-left-inset);
    border-color: var(--line);
    border-radius:
      max(0px, calc(var(--bevel-top-left-radius) - var(--bevel-face-width)))
      max(0px, calc(var(--bevel-top-right-radius) - var(--bevel-face-width)))
      max(0px, calc(var(--bevel-bottom-right-radius) - var(--bevel-face-width)))
      max(0px, calc(var(--bevel-bottom-left-radius) - var(--bevel-face-width)));
  }

  .bevel[data-profile='ridge'][data-depth='inset']::after {
    border-color: var(--line-strong);
  }

  .bevel[data-invalid='true'],
  .bevel:has(> :global([aria-invalid='true'])) {
    box-shadow: 0 0 0 1px var(--danger);
  }

  .bevel:has(> :global(:is(:disabled, [aria-disabled='true']))) {
    opacity: 0.58;
  }

  @supports
    (color: color-mix(in oklab, white 50%, black)) and
    (width: calc(cos(0deg) * 1px)) and
    ((mask-composite: exclude) or (-webkit-mask-composite: xor)) {
    .bevel {
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
    }

    .bevel::before,
    .bevel[data-profile='ridge']::after {
      --_W: var(--bevel-face-width);
      --_RTL: var(--bevel-top-left-radius);
      --_RTR: var(--bevel-top-right-radius);
      --_RBR: var(--bevel-bottom-right-radius);
      --_RBL: var(--bevel-bottom-left-radius);
      --_L: var(--bevel-light);
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

      border-style: solid;
      border-color: transparent;
      border-width: var(--_top-w) var(--_right-w) var(--_bottom-w) var(--_left-w);
      border-radius: var(--_RTL) var(--_RTR) var(--_RBR) var(--_RBL);
      background:
        conic-gradient(from var(--_L) at 100% 100%, var(--_shade)) 0 0 / var(--_RTL) var(--_RTL) no-repeat,
        conic-gradient(from var(--_L) at 0 100%, var(--_shade)) 100% 0 / var(--_RTR) var(--_RTR) no-repeat,
        conic-gradient(from var(--_L) at 0 0, var(--_shade)) 100% 100% / var(--_RBR) var(--_RBR) no-repeat,
        conic-gradient(from var(--_L) at 100% 0, var(--_shade)) 0 100% / var(--_RBL) var(--_RBL) no-repeat,
        linear-gradient(var(--_top) 0 0) var(--_RTL) 0 /
          calc(100% - var(--_RTL) - var(--_RTR)) var(--_top-w) no-repeat,
        linear-gradient(var(--_bottom) 0 0) var(--_RBL) 100% /
          calc(100% - var(--_RBL) - var(--_RBR)) var(--_bottom-w) no-repeat,
        linear-gradient(var(--_left) 0 0) 0 var(--_RTL) / var(--_left-w)
          calc(100% - var(--_RTL) - var(--_RBL)) no-repeat,
        linear-gradient(var(--_right) 0 0) 100% var(--_RTR) / var(--_right-w)
          calc(100% - var(--_RTR) - var(--_RBR)) no-repeat;
      background-origin: border-box;
      background-clip: border-box;
      box-shadow: none;
      -webkit-mask:
        linear-gradient(#000 0 0) padding-box,
        linear-gradient(#000 0 0);
      -webkit-mask-composite: xor;
      mask:
        linear-gradient(#000 0 0) padding-box,
        linear-gradient(#000 0 0);
      mask-composite: exclude;
    }

    .bevel[data-depth='inset']::before,
    .bevel[data-profile='edge'][data-pressed='true']::before,
    .bevel[data-profile='edge'][data-depth='raised']:has(
      > :global(:is(button, [role='button']):active)
    )::before,
    .bevel[data-profile='edge'][data-depth='raised']:has(
      > :global([aria-pressed='true'])
    )::before {
      --_L: calc(var(--bevel-light) + 180deg);
    }

    .bevel[data-profile='ridge']::before {
      --_W: var(--bevel-face-width);
    }

    .bevel[data-profile='ridge']::after {
      --_W: var(--bevel-face-width);
      --_RTL: max(0px, calc(var(--bevel-top-left-radius) - var(--bevel-face-width)));
      --_RTR: max(0px, calc(var(--bevel-top-right-radius) - var(--bevel-face-width)));
      --_RBR: max(0px, calc(var(--bevel-bottom-right-radius) - var(--bevel-face-width)));
      --_RBL: max(0px, calc(var(--bevel-bottom-left-radius) - var(--bevel-face-width)));
      inset:
        var(--bevel-top-inset)
        var(--bevel-right-inset)
        var(--bevel-bottom-inset)
        var(--bevel-left-inset);
    }

    .bevel[data-profile='ridge'][data-depth='raised']::after {
      --_L: calc(var(--bevel-light) + 180deg);
    }

    .bevel[data-profile='ridge'][data-depth='inset']::after {
      --_L: var(--bevel-light);
    }
  }

  @media (forced-colors: active) {
    .bevel {
      forced-color-adjust: auto;
    }

    .bevel::before,
    .bevel[data-profile='ridge']::after {
      border-color: ButtonText;
      background: none;
      box-shadow: none;
      -webkit-mask: none;
      mask: none;
    }

    .bevel[data-invalid='true'],
    .bevel:has(> :global([aria-invalid='true'])) {
      outline: 2px solid Mark;
      outline-offset: 1px;
      box-shadow: none;
    }
  }

  @media print {
    .bevel::before,
    .bevel[data-profile='ridge']::after {
      border-color: currentColor;
      background: none;
      -webkit-mask: none;
      mask: none;
    }
  }
</style>
