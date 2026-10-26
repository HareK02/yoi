// @vitest-environment happy-dom
import { afterEach, describe, expect, test, vi } from "vitest";
import { swipeSidebar } from "./swipe-sidebar.ts";

let destroy: (() => void) | undefined;
afterEach(() => {
  destroy?.();
  destroy = undefined;
  document.body.replaceChildren();
  document.getSelection()?.removeAllRanges();
  vi.restoreAllMocks();
});

describe.each([false, true])("sidebar initially open=%s", (initiallyOpen) => {
  function fixture(html = "<p>Page content</p>") {
    const node = document.createElement("div");
    node.innerHTML = html;
    document.body.append(node);
    let enabled = true;
    let isOpen = initiallyOpen;
    const open = vi.fn((next: boolean) => {
      isOpen = next;
    });
    const listeners = vi.spyOn(node, "addEventListener");
    destroy = swipeSidebar(node, {
      enabled: () => enabled,
      isOpen: () => isOpen,
      setOpen: open,
    }).destroy;
    const target = node.firstElementChild!;
    const send = (
      type: string,
      x: number,
      y: number,
      time = 100,
      count = 1,
      id = 1,
    ) => {
      const touch = {
        identifier: id,
        clientX: initiallyOpen ? window.innerWidth - x : x,
        clientY: y,
      };
      const event = new Event(type, { bubbles: true, cancelable: true });
      Object.defineProperties(event, {
        touches: { value: type === "touchend" ? [] : Array(count).fill(touch) },
        changedTouches: { value: [touch] },
        timeStamp: { value: time },
      });
      target.dispatchEvent(event);
      expect(event.defaultPrevented).toBe(false);
    };
    return {
      node,
      target,
      open,
      send,
      listeners,
      flipState: () => {
        isOpen = !isOpen;
      },
      disable: () => {
        enabled = false;
      },
    };
  }

  test("intentional swipe changes state on release without claiming native touch events", () => {
    const { node, open, send, listeners } = fixture();
    send("touchstart", 120, 200);
    send("touchmove", 150, 203, 150);
    send("touchmove", 230, 205, 200);
    expect(open).not.toHaveBeenCalled();
    send("touchend", 230, 205, 250);
    expect(open).toHaveBeenCalledTimes(1);
    expect(open).toHaveBeenCalledWith(!initiallyOpen);
    expect(node.style.touchAction).toBe("");
    for (const [name, , options] of listeners.mock.calls) {
      if (String(name).startsWith("touch")) {
        expect(options).toEqual({ passive: true });
      }
    }
  });

  for (
    const [name, points] of [
      ["tap", [[120, 200], [122, 201]]],
      ["short horizontal motion", [[120, 200], [170, 202]]],
      ["vertical scroll", [[120, 200], [125, 250], [240, 252]]],
      ["diagonal motion", [[120, 200], [140, 214], [230, 218]]],
      ["opposite direction", [[240, 200], [210, 202], [120, 203]]],
      ["vertical excursion before returning", [
        [120, 200],
        [150, 202],
        [230, 240],
        [240, 201],
      ]],
      ["browser left edge", [[10, 200], [150, 201]]],
    ] as const
  ) {
    test(`${name} does not open the sidebar`, () => {
      const { open, send } = fixture();
      points.forEach(([x, y], i) =>
        send(i === 0 ? "touchstart" : "touchmove", x, y, 100 + i * 30)
      );
      const [x, y] = points[points.length - 1];
      send("touchend", x, y, 300);
      expect(open).not.toHaveBeenCalled();
    });
  }

  for (
    const html of [
      '<input type="range">',
      "<textarea></textarea>",
      "<select></select>",
      "<button>Action</button>",
      '<div contenteditable="true">Editor</div>',
      '<div role="slider">Slider</div>',
      "<dialog open>Dialog</dialog>",
    ]
  ) {
    test(`does not claim control gestures: ${html}`, () => {
      const { open, send } = fixture(html);
      send("touchstart", 120, 200);
      send("touchend", 240, 201, 300);
      expect(open).not.toHaveBeenCalled();
    });
  }

  test("horizontal scroll container remains excluded even at its boundary", () => {
    const { node, open, send } = fixture();
    node.style.overflowX = "auto";
    Object.defineProperty(node, "scrollWidth", { value: 800 });
    Object.defineProperty(node, "clientWidth", { value: 300 });
    send("touchstart", 120, 200);
    send("touchend", 240, 201, 300);
    expect(open).not.toHaveBeenCalled();
  });

  for (
    const reason of [
      "scroll",
      "touchcancel",
      "multitouch",
      "disabled",
      "state-change",
      "held",
      "identifier",
      "destroy",
      "selection",
    ]
  ) {
    test(`abandons the gesture after ${reason}`, () => {
      const { target, open, send, disable, flipState } = fixture();
      send("touchstart", 120, 200);
      send("touchmove", 160, 202, 150);
      if (reason === "scroll" || reason === "touchcancel") {
        target.dispatchEvent(
          new Event(reason, { bubbles: reason === "touchcancel" }),
        );
      }
      if (reason === "multitouch") {
        send("touchstart", 160, 202, 160, 2);
      }
      if (reason === "disabled") disable();
      if (reason === "state-change") flipState();
      if (reason === "destroy") destroy?.();
      if (reason === "selection") {
        const range = document.createRange();
        range.selectNodeContents(target);
        document.getSelection()?.addRange(range);
      }
      send(
        "touchend",
        240,
        203,
        reason === "held" ? 1000 : 300,
        1,
        reason === "identifier" ? 2 : 1,
      );
      expect(open).not.toHaveBeenCalled();
    });
  }

  test("a tap on a link is untouched while an intentional swipe over text can open", () => {
    const { target, open, send } = fixture('<a href="#link">Link text</a>');
    const click = new Event("click", { bubbles: true, cancelable: true });
    target.dispatchEvent(click);
    expect(click.defaultPrevented).toBe(false);
    send("touchstart", 120, 200);
    send("touchend", 240, 201, 300);
    expect(open).toHaveBeenCalledOnce();
  });
});
