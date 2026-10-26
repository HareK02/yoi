type Options = {
  enabled: boolean;
  open: boolean;
  route: string;
  setOpen: (open: boolean) => void;
};

type Sample = { x: number; time: number };
const SLOP = 10;
const AXIS_RATIO = 1.5;
const MIN_FLING_DISTANCE = 24;
const FLING_VELOCITY = 450; // CSS px/s, measured over the last 120ms.
const COMMIT_FRACTION = 0.4;

// Leave undecided/vertical gestures to the browser. Only a confirmed horizontal
// drag claims touchmove; no ancestor touch-action restriction disables nested
// horizontal scrolling or pinch zoom. Preview never changes admission/focus.
export function swipeSidebar(node: HTMLElement, options: Options) {
  let gesture: {
    id: number;
    x: number;
    y: number;
    horizontal: boolean;
    wasOpen: boolean;
    width: number;
    startOffset: number;
    offset: number;
    peak: number;
    samples: Sample[];
  } | null = null;
  let suppressedClick: { x: number; y: number; until: number } | null = null;
  let cleanupVersion = 0;
  const view = node.ownerDocument.defaultView;
  const selectedText = () =>
    node.ownerDocument.getSelection()?.isCollapsed === false;
  const clearPreview = () => {
    node.classList.remove("sidebar-swiping");
    node.style.removeProperty("--sidebar-swipe-offset");
  };
  const cancel = () => {
    gesture = null;
    node.removeEventListener("touchmove", move);
    cleanupVersion++;
    clearPreview();
  };

  function excluded(target: EventTarget | null): boolean {
    let element = target instanceof Element ? target : null;
    if (
      !element || element.closest(
        'input, textarea, select, [contenteditable]:not([contenteditable="false"]), [role="slider"], [role="spinbutton"], [role="scrollbar"], [role="dialog"], [role="menu"], [role="listbox"], dialog, video, audio, canvas, [draggable="true"], [data-sidebar-swipe-ignore]',
      )
    ) return true;
    // A horizontal scroll surface owns the whole gesture, including its edges.
    for (; element; element = element.parentElement) {
      if (
        element.scrollWidth > element.clientWidth + 1 &&
        /^(auto|scroll|overlay)$/.test(getComputedStyle(element).overflowX)
      ) return true;
      if (element === node) break;
    }
    return false;
  }

  function start(event: TouchEvent) {
    cancel();
    suppressedClick = null; // A new tap must not inherit the previous swipe.
    if (
      !options.enabled || event.defaultPrevented ||
      event.touches.length !== 1 || selectedText() || excluded(event.target)
    ) return;
    const touch = event.touches[0];
    const viewportWidth = view?.innerWidth ?? 0;
    if (touch.clientX < 24 || touch.clientX > viewportWidth - 24) return;
    const panel = node.querySelector<HTMLElement>(".sidebar-frame");
    if (!panel) return;
    const bounds = panel.getBoundingClientRect();
    if (bounds.width <= 0) return;
    const offset = Math.min(
      0,
      Math.max(-bounds.width, bounds.left - node.getBoundingClientRect().left),
    );
    gesture = {
      id: touch.identifier,
      x: touch.clientX,
      y: touch.clientY,
      horizontal: false,
      wasOpen: options.open,
      width: bounds.width,
      startOffset: offset,
      offset,
      peak: 0,
      samples: [{ x: 0, time: event.timeStamp }],
    };
    // Install a potentially blocking listener only for an eligible gesture.
    node.addEventListener("touchmove", move, { passive: false });
  }

  function track(event: TouchEvent, touch: Touch): boolean {
    const current = gesture;
    if (!current) return false;
    if (
      !options.enabled || options.open !== current.wasOpen ||
      event.defaultPrevented || selectedText() ||
      touch.identifier !== current.id
    ) {
      cancel();
      return false;
    }
    const rawDx = touch.clientX - current.x;
    const dx = rawDx * (current.wasOpen ? -1 : 1);
    const dy = Math.abs(touch.clientY - current.y);
    if (!current.horizontal) {
      if (
        (dy >= SLOP && dy > Math.abs(dx) * AXIS_RATIO) ||
        (dx <= -SLOP && -dx > dy * AXIS_RATIO)
      ) {
        cancel();
        return false;
      }
      // An ambiguous diagonal is not a failed swipe. Keep observing until
      // intent becomes clear, unless the browser starts scrolling first.
      if (dx < SLOP || dx < dy * AXIS_RATIO) return false;
      if (!event.cancelable) {
        cancel(); // Native scrolling already owns this stream.
        return false;
      }
      current.horizontal = true;
    }
    if (!event.cancelable && event.type !== "touchend") {
      cancel();
      return false;
    }
    if (event.cancelable) event.preventDefault();
    current.offset = Math.min(
      0,
      Math.max(-current.width, current.startOffset + rawDx),
    );
    current.peak = Math.max(current.peak, dx);
    current.samples.push({ x: dx, time: event.timeStamp });
    current.samples = current.samples.filter((sample) =>
      sample.time >= event.timeStamp - 120
    );
    suppressedClick = {
      x: touch.clientX,
      y: touch.clientY,
      until: performance.now() + 750,
    };
    node.style.setProperty("--sidebar-swipe-offset", `${current.offset}px`);
    node.classList.add("sidebar-swiping");
    return true;
  }

  function move(event: TouchEvent) {
    if (event.touches.length !== 1) {
      cancel();
      return;
    }
    track(event, event.touches[0]);
  }

  function end(event: TouchEvent) {
    // Do not turn an unclaimed touch into a swipe only after the finger lifts.
    if (
      !gesture?.horizontal || event.touches.length !== 0 ||
      event.changedTouches.length !== 1 ||
      !track(event, event.changedTouches[0])
    ) {
      cancel();
      return;
    }
    const current = gesture;
    const last = current.samples.at(-1)!;
    const first = current.samples[0];
    const duration = last.time - first.time;
    const velocity = duration >= 8 ? (last.x - first.x) * 1000 / duration : 0;
    const progress = current.wasOpen
      ? -current.offset
      : current.width + current.offset;
    const reversed = velocity <= -FLING_VELOCITY &&
      current.peak - last.x >= MIN_FLING_DISTANCE;
    const commit = !reversed && (
      progress >= current.width * COMMIT_FRACTION ||
      (last.x >= MIN_FLING_DISTANCE && velocity >= FLING_VELOCITY)
    );
    gesture = null;
    node.removeEventListener("touchmove", move);
    if (commit) options.setOpen(!current.wasOpen);
    // Let Svelte apply the final open/closed class before removing the preview;
    // CSS then settles from the actual dragged position, including snap-back.
    const version = ++cleanupVersion;
    queueMicrotask(() => {
      if (version === cleanupVersion) clearPreview();
    });
  }

  function click(event: MouseEvent) {
    if (!suppressedClick || performance.now() > suppressedClick.until) return;
    // Keep keyboard activation and a subsequent deliberate tap intact.
    if (event.detail === 0) return;
    if (
      Math.hypot(
        event.clientX - suppressedClick.x,
        event.clientY - suppressedClick.y,
      ) > 32
    ) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    suppressedClick = null;
  }

  const passive = { passive: true };
  node.addEventListener("touchstart", start, passive);
  node.addEventListener("touchend", end, { passive: false });
  node.addEventListener("touchcancel", cancel, passive);
  node.addEventListener("scroll", cancel, { passive: true, capture: true });
  node.addEventListener("click", click, true);
  view?.addEventListener("resize", cancel);
  node.ownerDocument.addEventListener("visibilitychange", cancel);
  return {
    update(next: Options) {
      if (
        !next.enabled || next.open !== options.open ||
        next.route !== options.route
      ) cancel();
      options = next;
    },
    destroy() {
      cancel();
      node.removeEventListener("touchstart", start);
      node.removeEventListener("touchend", end);
      node.removeEventListener("touchcancel", cancel);
      node.removeEventListener("scroll", cancel, true);
      node.removeEventListener("click", click, true);
      view?.removeEventListener("resize", cancel);
      node.ownerDocument.removeEventListener("visibilitychange", cancel);
    },
  };
}
