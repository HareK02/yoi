<script lang="ts">
  type TimelineKind = 'turn' | 'assistant';

  export type TimelineMark = {
    id: string;
    lineId: string;
    label: string;
    detail: string;
    timeLabel: string;
    position: number;
    sourcePosition: number;
    kind: TimelineKind;
  };

  type Props = {
    marks: TimelineMark[];
    thumbStyle: string;
    axisStyle: string;
    expanded: boolean;
    onRailPointerDown: (event: PointerEvent) => void;
    onMarkClick: (mark: TimelineMark) => void;
  };

  let {
    marks,
    thumbStyle,
    axisStyle,
    expanded,
    onRailPointerDown,
    onMarkClick,
  }: Props = $props();
</script>

<aside class="console-timeline" aria-label="Console timeline">
  <div class="timeline-axis" style={axisStyle}>
    <button
      type="button"
      class="timeline-rail"
      aria-label="Scroll console"
      onpointerdown={onRailPointerDown}
    >
      <span class="timeline-thumb" style={thumbStyle}></span>
    </button>
    <div class="timeline-marks" aria-label="Timeline marks">
      {#each marks as mark (mark.id)}
        <button
          type="button"
          class={`timeline-mark ${mark.kind} ${expanded ? 'expanded' : 'folded'}`}
          style={`top: ${mark.position}px`}
          aria-label={mark.detail ? `${mark.label}: ${mark.detail}` : mark.label}
          onclick={() => onMarkClick(mark)}
        >
          <span class="timeline-dot"></span>
          <span class="timeline-card">
            <span>{mark.label}</span>
          </span>
        </button>
      {/each}
    </div>
  </div>
</aside>

<style>
  .console-timeline {
    --timeline-axis-padding: 3rem;

    position: relative;
    grid-column: 2;
    grid-row: 2;
    width: 100%;
    min-width: 0;
    min-height: 0;
    height: 100%;
    opacity: 1;
  }

  .timeline-axis {
    position: absolute;
    right: 0;
    left: 0;
  }

  .timeline-rail,
  .timeline-marks {
    position: absolute;
    top: 0;
    bottom: 0;
    left: 0;
  }

  .timeline-rail {
    width: 0.65rem;
    border: 0;
    border-radius: 999px;
    background: color-mix(in srgb, var(--line) 70%, transparent);
    cursor: ns-resize;
    padding: 0;
    touch-action: none;
    user-select: none;
  }

  .timeline-thumb {
    position: absolute;
    left: 0;
    width: 100%;
    min-height: 1.25rem;
    border-radius: inherit;
    background: var(--tui-cyan);
    box-shadow: 0 0 0 1px color-mix(in srgb, var(--tui-cyan) 30%, transparent);
    pointer-events: none;
  }

  .timeline-marks {
    width: 12rem;
    pointer-events: none;
  }

  .timeline-mark {
    position: absolute;
    left: 0;
    display: flex;
    width: 11.5rem;
    align-items: center;
    gap: var(--space-2);
    border: 0;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
    padding: 0;
    pointer-events: auto;
    transform: translateY(-50%);
  }

  .timeline-dot {
    width: 0.6rem;
    height: 0.6rem;
    flex: 0 0 auto;
    border: 1px solid var(--bg);
    border-radius: 999px;
    background: var(--tui-gray);
    box-shadow: 0 0 0 1px color-mix(in srgb, var(--line) 80%, transparent);
  }

  .timeline-mark.turn .timeline-dot {
    background: var(--tui-blue);
  }

  .timeline-mark.assistant .timeline-dot {
    background: var(--tui-green);
  }

  .timeline-card {
    display: none;
    width: 10rem;
    min-width: 0;
    grid-template-columns: minmax(0, 1fr);
    justify-items: start;
    text-align: left;
    border: 1px solid var(--line);
    border-radius: 10px;
    background: color-mix(in srgb, var(--bg-panel) 92%, transparent);
    box-shadow: var(--shadow-soft);
    opacity: 1;
    padding: 0.25rem 0.45rem;
    transform: none;
  }

  .timeline-mark.expanded .timeline-card {
    display: grid;
  }

  .timeline-card span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .timeline-mark.turn .timeline-card {
    border-color: color-mix(in srgb, var(--tui-blue) 55%, var(--line));
  }

  .timeline-mark.assistant .timeline-card {
    border-color: color-mix(in srgb, var(--tui-green) 55%, var(--line));
  }
</style>
