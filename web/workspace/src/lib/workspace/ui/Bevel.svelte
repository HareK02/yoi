<script lang="ts">
  import type { Snippet } from 'svelte';

  type Props = {
    children: Snippet<[]>;
    top?: boolean;
    right?: boolean;
    bottom?: boolean;
    left?: boolean;
    fill?: boolean;
    invalid?: boolean;
    as?: 'span' | 'div';
    class?: string;
  };

  let {
    children,
    top = true,
    right = true,
    bottom = true,
    left = true,
    fill = false,
    invalid = false,
    as = 'span',
    class: className = '',
  }: Props = $props();
</script>

<svelte:element
  this={as}
  class={`bevel ${fill ? 'bevel--fill' : ''} ${className}`.trim()}
  data-top={top ? 'true' : 'false'}
  data-right={right ? 'true' : 'false'}
  data-bottom={bottom ? 'true' : 'false'}
  data-left={left ? 'true' : 'false'}
  data-invalid={invalid || undefined}
>
  {@render children()}
</svelte:element>

<style>
  .bevel {
    --bevel-radius: var(--radius-soft);
    --bevel-top-left-radius: var(--bevel-radius);
    --bevel-top-right-radius: var(--bevel-radius);
    --bevel-bottom-right-radius: var(--bevel-radius);
    --bevel-bottom-left-radius: var(--bevel-radius);

    display: inline-grid;
    min-width: 0;
    border-style: solid;
    border-color: var(--line-strong);
    border-width: 1px;
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

    border-top-width: 0;
  }

  .bevel[data-right='false'] {
    --bevel-top-right-radius: 0px;
    --bevel-bottom-right-radius: 0px;

    border-right-width: 0;
  }

  .bevel[data-bottom='false'] {
    --bevel-bottom-right-radius: 0px;
    --bevel-bottom-left-radius: 0px;

    border-bottom-width: 0;
  }

  .bevel[data-left='false'] {
    --bevel-bottom-left-radius: 0px;
    --bevel-top-left-radius: 0px;

    border-left-width: 0;
  }

  .bevel--fill {
    width: 100%;
  }

  .bevel > :global(*) {
    min-width: 0;
    border-radius: inherit;
  }

  .bevel > :global(:is(button, input, select, textarea)) {
    border-color: transparent;
  }

  .bevel[data-invalid='true'],
  .bevel:has(> :global([aria-invalid='true'])) {
    border-color: var(--danger);
  }

  .bevel:has(> :global(:is(:disabled, [aria-disabled='true']))) {
    opacity: 0.58;
  }

  @media (forced-colors: active) {
    .bevel {
      border-color: ButtonText;
      forced-color-adjust: auto;
    }

    .bevel[data-invalid='true'],
    .bevel:has(> :global([aria-invalid='true'])) {
      border-color: Mark;
    }
  }

  @media print {
    .bevel {
      border-color: currentColor;
    }
  }
</style>
