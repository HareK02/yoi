type Options = {
  enabled: () => boolean;
  isOpen: () => boolean;
  setOpen: (open: boolean) => void;
};

// Observe, never claim, the touch stream: native scrolling/zoom and child controls
// retain their defaults. A deliberate right swipe opens; a left swipe closes.
export function swipeSidebar(node: HTMLElement, options: Options) {
  let gesture: {
    id: number;
    x: number;
    y: number;
    started: number;
    horizontal: boolean;
    wasOpen: boolean;
  } | null = null;
  const cancel = () => {
    gesture = null;
  };
  const selectedText = () =>
    node.ownerDocument.getSelection()?.isCollapsed === false;

  function excluded(target: EventTarget | null): boolean {
    let element = target instanceof Element ? target : null;
    if (
      !element || element.closest(
        'input, textarea, select, button, [contenteditable]:not([contenteditable="false"]), [role="slider"], [role="spinbutton"], [role="scrollbar"], [role="dialog"], [role="menu"], [role="listbox"], dialog, video, audio, canvas, [draggable="true"]',
      )
    ) return true;
    // Do not steal a horizontal pan even at a scroll container's left/right end.
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
    if (
      !options.enabled() || event.defaultPrevented ||
      event.touches.length !== 1 ||
      selectedText() || excluded(event.target)
    ) return;
    const touch = event.touches[0];
    const width = node.ownerDocument.defaultView?.innerWidth ?? 0;
    // Leave OS/browser edge-navigation gestures alone.
    if (touch.clientX < 24 || touch.clientX > width - 24) return;
    gesture = {
      id: touch.identifier,
      x: touch.clientX,
      y: touch.clientY,
      started: event.timeStamp,
      horizontal: false,
      wasOpen: options.isOpen(),
    };
  }

  function track(event: TouchEvent, touch: Touch): boolean {
    if (!gesture) return false;
    const dx = (touch.clientX - gesture.x) * (gesture.wasOpen ? -1 : 1);
    const dy = Math.abs(touch.clientY - gesture.y);
    if (
      !options.enabled() || options.isOpen() !== gesture.wasOpen ||
      event.defaultPrevented || selectedText() ||
      touch.identifier !== gesture.id ||
      event.timeStamp - gesture.started > 700 ||
      dy > 32 || dx < -12
    ) {
      cancel();
      return false;
    }
    if (!gesture.horizontal && Math.hypot(dx, dy) >= 12) {
      if (dx < 2.5 * dy) {
        cancel();
        return false;
      }
      gesture.horizontal = true;
    }
    return gesture.horizontal && dx >= 80 && dx >= 2.5 * dy;
  }

  function move(event: TouchEvent) {
    if (event.touches.length !== 1) {
      cancel();
      return;
    }
    track(event, event.touches[0]);
  }

  function end(event: TouchEvent) {
    const nextOpen = !gesture?.wasOpen;
    const accepted = event.touches.length === 0 &&
      event.changedTouches.length === 1 &&
      track(event, event.changedTouches[0]);
    cancel();
    if (accepted) options.setOpen(nextOpen);
  }

  const passive = { passive: true };
  node.addEventListener("touchstart", start, passive);
  node.addEventListener("touchmove", move, passive);
  node.addEventListener("touchend", end, passive);
  node.addEventListener("touchcancel", cancel, passive);
  node.addEventListener("scroll", cancel, { passive: true, capture: true });
  return {
    destroy() {
      cancel();
      node.removeEventListener("touchstart", start);
      node.removeEventListener("touchmove", move);
      node.removeEventListener("touchend", end);
      node.removeEventListener("touchcancel", cancel);
      node.removeEventListener("scroll", cancel, true);
    },
  };
}
