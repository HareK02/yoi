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
    const target = node.firstElementChild!;
    const panel = document.createElement("aside");
    panel.className = "sidebar-frame";
    node.append(panel);
    document.body.append(node);
    let enabled = true;
    let isOpen = initiallyOpen;
    let route = "/account";
    vi.spyOn(panel, "getBoundingClientRect").mockImplementation(() => ({
      width: 300,
      left: isOpen ? 0 : -300,
    } as DOMRect));
    const open = vi.fn((next: boolean) => {
      isOpen = next;
      action.update(options());
    });
    const options = () => ({ enabled, open: isOpen, route, setOpen: open });
    const listeners = vi.spyOn(node, "addEventListener");
    const action = swipeSidebar(node, options());
    destroy = action.destroy;
    const clientX = (x: number) => initiallyOpen ? window.innerWidth - x : x;
    const send = (
      type: string,
      x: number,
      y: number,
      time = 100,
      count = 1,
      id = 1,
      cancelable = true,
    ) => {
      const touch = { identifier: id, clientX: clientX(x), clientY: y };
      const event = new Event(type, { bubbles: true, cancelable });
      Object.defineProperties(event, {
        touches: { value: type === "touchend" ? [] : Array(count).fill(touch) },
        changedTouches: { value: [touch] },
        timeStamp: { value: time },
      });
      target.dispatchEvent(event);
      return event;
    };
    const click = (x: number, y: number, detail = 1) => {
      const event = new MouseEvent("click", {
        bubbles: true,
        cancelable: true,
        detail,
        clientX: clientX(x),
        clientY: y,
      });
      target.dispatchEvent(event);
      return event;
    };
    return {
      node,
      target,
      panel,
      open,
      send,
      click,
      listeners,
      flipState: () => {
        isOpen = !isOpen;
        action.update(options());
      },
      disable: () => {
        enabled = false;
        action.update(options());
      },
      navigate: () => {
        route = "/other";
        action.update(options());
      },
    };
  }

  test("horizontal intent previews position, then settles only on release", async () => {
    const { node, open, send, listeners } = fixture();
    expect(send("touchstart", 120, 200).defaultPrevented).toBe(false);
    expect(node.classList.contains("sidebar-swiping")).toBe(false);
    expect(send("touchmove", 150, 203, 150).defaultPrevented).toBe(true);
    expect(node.style.getPropertyValue("--sidebar-swipe-offset")).toBe(
      initiallyOpen ? "-30px" : "-270px",
    );
    send("touchmove", 270, 205, 500);
    expect(open).not.toHaveBeenCalled();
    expect(node.style.getPropertyValue("--sidebar-swipe-offset")).toBe(
      "-150px",
    );
    send("touchend", 270, 205, 900);
    expect(open).toHaveBeenCalledExactlyOnceWith(!initiallyOpen);
    await Promise.resolve();
    expect(node.classList.contains("sidebar-swiping")).toBe(false);
    expect(node.style.getPropertyValue("--sidebar-swipe-offset")).toBe("");
    expect(node.style.touchAction).toBe("");
    expect(listeners.mock.calls.find(([name]) => name === "touchmove")?.[2])
      .toEqual({ passive: false });
  });

  test("ambiguous start remains pending; horizontal lock tolerates an arcing thumb", () => {
    const { node, open, send } = fixture();
    send("touchstart", 120, 200);
    expect(send("touchmove", 132, 210, 120).defaultPrevented).toBe(false);
    expect(node.classList.contains("sidebar-swiping")).toBe(false);
    expect(send("touchmove", 160, 214, 170).defaultPrevented).toBe(true);
    send("touchmove", 280, 260, 500);
    send("touchend", 280, 260, 700);
    expect(open).toHaveBeenCalledExactlyOnceWith(!initiallyOpen);
  });

  test("short fast flick commits without the former 80px requirement", () => {
    const { open, send } = fixture();
    send("touchstart", 120, 200);
    send("touchmove", 132, 202, 115);
    send("touchmove", 156, 203, 150);
    send("touchend", 156, 203, 160);
    expect(open).toHaveBeenCalledExactlyOnceWith(!initiallyOpen);
  });

  test("a noncancelable release still settles an already claimed drag", () => {
    const { open, send } = fixture();
    send("touchstart", 120, 200);
    send("touchmove", 280, 203, 300);
    send("touchend", 280, 203, 400, 1, 1, false);
    expect(open).toHaveBeenCalledExactlyOnceWith(!initiallyOpen);
  });

  test("slow long drag still commits after 700ms", () => {
    const { open, send } = fixture();
    send("touchstart", 120, 200);
    send("touchmove", 150, 202, 400);
    send("touchmove", 280, 203, 1500);
    send("touchend", 280, 203, 1800);
    expect(open).toHaveBeenCalledExactlyOnceWith(!initiallyOpen);
  });

  for (
    const [name, points] of [
      ["tap", [[120, 200, 100], [122, 201, 130]]],
      ["short slow drag", [[120, 200, 100], [160, 202, 500]]],
      ["tiny fast flick", [[120, 200, 100], [138, 202, 120]]],
      ["vertical intent", [[120, 200, 100], [125, 240, 150], [280, 242, 200]]],
      ["ambiguous diagonal throughout", [[120, 200, 100], [135, 215, 130], [
        200,
        270,
        200,
      ]]],
      ["opposite direction", [[240, 200, 100], [210, 202, 140], [
        120,
        203,
        200,
      ]]],
      ["browser edge", [[10, 200, 100], [180, 201, 300]]],
      ["fast motion followed by a pause", [[120, 200, 100], [160, 202, 150], [
        160,
        202,
        600,
      ]]],
      ["drag returned to start", [[120, 200, 100], [300, 202, 300], [
        125,
        203,
        900,
      ]]],
      ["reverse flick overrides distance", [[120, 200, 100], [320, 202, 200], [
        320,
        202,
        380,
      ], [260, 203, 430]]],
    ] as const
  ) {
    test(`${name} leaves the sidebar in its original state`, async () => {
      const { node, open, send } = fixture();
      points.forEach(([x, y, time], i) =>
        send(i === 0 ? "touchstart" : "touchmove", x, y, time)
      );
      const [x, y, time] = points.at(-1)!;
      send("touchend", x, y, time + 10);
      await Promise.resolve();
      expect(open).not.toHaveBeenCalled();
      expect(node.classList.contains("sidebar-swiping")).toBe(false);
    });
  }

  for (
    const html of [
      '<input type="range">',
      "<textarea></textarea>",
      "<select></select>",
      '<div contenteditable="true">Editor</div>',
      '<div role="slider">Slider</div>',
      "<dialog open>Dialog</dialog>",
      "<div data-sidebar-swipe-ignore>Custom gesture</div>",
    ]
  ) {
    test(`keeps native controls unclaimed: ${html}`, () => {
      const { open, send, listeners } = fixture(html);
      send("touchstart", 120, 200);
      expect(send("touchmove", 290, 201, 300).defaultPrevented).toBe(false);
      send("touchend", 290, 201, 310);
      expect(open).not.toHaveBeenCalled();
      expect(listeners.mock.calls.some(([name]) => name === "touchmove")).toBe(
        false,
      );
    });
  }

  test("nested horizontal scrolling remains excluded even at its boundary", () => {
    const { target, open, send } = fixture();
    (target as HTMLElement).style.overflowX = "auto";
    Object.defineProperty(target, "scrollWidth", { value: 800 });
    Object.defineProperty(target, "clientWidth", { value: 300 });
    send("touchstart", 120, 200);
    expect(send("touchmove", 290, 201, 300).defaultPrevented).toBe(false);
    send("touchend", 290, 201, 310);
    expect(open).not.toHaveBeenCalled();
  });

  for (
    const reason of [
      "scroll",
      "touchcancel",
      "multitouch",
      "disabled",
      "state-change",
      "navigation",
      "resize",
      "hidden",
      "identifier",
      "destroy",
      "selection",
      "native-claimed",
    ]
  ) {
    test(`cancels preview after ${reason}`, () => {
      const { node, target, open, send, disable, flipState, navigate } =
        fixture();
      send("touchstart", 120, 200);
      send("touchmove", 160, 202, 150);
      expect(node.classList.contains("sidebar-swiping")).toBe(true);
      if (reason === "scroll" || reason === "touchcancel") {
        target.dispatchEvent(
          new Event(reason, { bubbles: reason === "touchcancel" }),
        );
      }
      if (reason === "multitouch") send("touchstart", 160, 202, 160, 2);
      if (reason === "disabled") disable();
      if (reason === "state-change") flipState();
      if (reason === "navigation") navigate();
      if (reason === "resize") window.dispatchEvent(new Event("resize"));
      if (reason === "hidden") {
        document.dispatchEvent(new Event("visibilitychange"));
      }
      if (reason === "destroy") destroy?.();
      if (reason === "selection") {
        const range = document.createRange();
        range.selectNodeContents(target);
        document.getSelection()?.addRange(range);
      }
      if (reason === "native-claimed") {
        send("touchmove", 200, 203, 200, 1, 1, false);
      }
      send("touchend", 290, 203, 300, 1, reason === "identifier" ? 2 : 1);
      expect(open).not.toHaveBeenCalled();
      expect(node.classList.contains("sidebar-swiping")).toBe(false);
    });
  }

  for (const html of ['<a href="#link">Link</a>', "<button>Action</button>"]) {
    test(`swipe from ${html} suppresses only its click, not later taps or keyboard activation`, async () => {
      const { open, send, click, target } = fixture(html);
      const activate = vi.fn();
      target.addEventListener("click", activate);
      send("touchstart", 120, 200);
      send("touchmove", 156, 202, 140);
      send("touchend", 156, 202, 150);
      await Promise.resolve();
      expect(open).toHaveBeenCalledOnce();
      expect(click(156, 202).defaultPrevented).toBe(true);
      expect(activate).not.toHaveBeenCalled();
      expect(click(156, 202, 0).defaultPrevented).toBe(false);
      send("touchstart", 160, 200, 200);
      send("touchend", 160, 200, 250);
      expect(click(160, 200).defaultPrevented).toBe(false);
      expect(activate).toHaveBeenCalledTimes(2);
    });
  }

  test("a claimed but incomplete swipe also suppresses accidental activation", async () => {
    const { open, send, click } = fixture("<button>Action</button>");
    send("touchstart", 120, 200);
    send("touchmove", 160, 202, 400);
    send("touchend", 160, 202, 500);
    await Promise.resolve();
    expect(open).not.toHaveBeenCalled();
    expect(click(160, 202).defaultPrevented).toBe(true);
  });
});
