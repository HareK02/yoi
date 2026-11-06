<script lang="ts">
  import type { FailureNoticeModel } from './model.ts';

  let { failure }: { failure: FailureNoticeModel } = $props();
</script>

<div class="failure-notice" role="alert" aria-atomic="true">
  <p class="failure-notice__cause"><strong>Operation failed</strong> — {failure.explanation}</p>
  {#if failure.compensation.length > 0}
    <div class="failure-notice__compensation">
      <strong>Cleanup needs attention</strong>
      <ul>{#each failure.compensation as explanation}<li>{explanation}</li>{/each}</ul>
    </div>
  {/if}
  {#if failure.supplements.length > 0}
    <div class="failure-notice__supplements">
      <strong>Additional information</strong>
      <ul>{#each failure.supplements as explanation}<li>{explanation}</li>{/each}</ul>
    </div>
  {/if}
  {#if failure.details.length > 0}
    <details>
      <summary>Failure details</summary>
      <dl>
        {#each failure.details as detail}
          <dt>{detail.label}</dt><dd>{detail.value}</dd>
        {/each}
      </dl>
    </details>
  {/if}
</div>

<style>
  .failure-notice {
    border-inline-start: 3px solid var(--danger);
    padding: 0.75rem 1rem;
    color: var(--text);
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .failure-notice__cause { margin: 0; }
  .failure-notice__cause strong { color: var(--danger); }
  .failure-notice__compensation, .failure-notice__supplements, details { margin-top: 0.75rem; }
  ul { padding-inline-start: 1.25rem; margin-block: 0.25rem 0; }
  summary { cursor: pointer; width: fit-content; }
  summary:focus-visible { outline: 2px solid currentColor; outline-offset: 3px; }
  dl { margin-bottom: 0; }
  dt { font-weight: 600; }
  dd { margin-inline-start: 1rem; margin-bottom: 0.5rem; }
</style>
