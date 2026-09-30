// @vitest-environment happy-dom

import { cleanup, fireEvent, render } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import ConsoleTurnNavigation from "./ConsoleTurnNavigation.svelte";

const items = [
  { turnId: "u1", lineId: "u1", user: "First question", assistant: ["First answer"] },
  { turnId: "u2", lineId: "u2", user: "Second question", assistant: [] },
];

afterEach(cleanup);

test("renders one bar per turn and previews both roles on hover", async () => {
  const view = render(ConsoleTurnNavigation, { items, onTurnClick: vi.fn() });
  expect(view.getAllByRole("button")).toHaveLength(2);
  expect(view.queryByRole("tooltip")).toBeNull();
  const button = view.getAllByRole("button")[0];
  await fireEvent.mouseEnter(button);
  const tooltip = view.getByRole("tooltip");
  expect(tooltip.textContent).toContain("First question");
  expect(tooltip.textContent).toContain("First answer");
  expect(button.getAttribute("aria-describedby")).toBe(tooltip.id);
  await fireEvent.mouseLeave(view.getByRole("navigation"));
  expect(view.queryByRole("tooltip")).toBeNull();
});

test("focus previews unanswered items, click selects the user anchor, Escape dismisses", async () => {
  const onTurnClick = vi.fn();
  const view = render(ConsoleTurnNavigation, { items, onTurnClick });
  const button = view.getAllByRole("button")[1];
  await fireEvent.focus(button);
  expect(view.getByRole("tooltip").textContent).toContain("No response yet");
  await fireEvent.click(button);
  expect(onTurnClick).toHaveBeenCalledWith(items[1]);
  await fireEvent.keyDown(window, { key: "Escape" });
  expect(view.queryByRole("tooltip")).toBeNull();
  await fireEvent.blur(button);
  await fireEvent.focus(button);
  expect(view.getByRole("tooltip")).toBeTruthy();
  await fireEvent.blur(button);
  expect(view.queryByRole("tooltip")).toBeNull();
});

test("updates the visible response in place and removes stale previews on history replacement", async () => {
  const onTurnClick = vi.fn();
  const view = render(ConsoleTurnNavigation, { items, onTurnClick });
  await fireEvent.mouseEnter(view.getAllByRole("button")[1]);
  await view.rerender({ items: [items[0], { ...items[1], assistant: ["Streaming answer"] }], onTurnClick });
  expect(view.getByRole("tooltip").textContent).toContain("Streaming answer");
  expect(view.getAllByRole("button")).toHaveLength(2);
  await view.rerender({ items: [], onTurnClick });
  expect(view.queryByRole("tooltip")).toBeNull();
  expect(view.queryAllByRole("button")).toHaveLength(0);
});

test("preview content stays text and scrolling the rail dismisses its old position", async () => {
  const view = render(ConsoleTurnNavigation, {
    items: [{ turnId: "u", lineId: "u", user: "<img src=x onerror=alert(1)>", assistant: ["<script>bad()</script>"] }],
    onTurnClick: vi.fn(),
  });
  await fireEvent.mouseEnter(view.getByRole("button"));
  const tooltip = view.getByRole("tooltip");
  expect(tooltip.querySelector("img, script")).toBeNull();
  expect(tooltip.textContent).toContain("<script>bad()</script>");
  await fireEvent.scroll(view.container.querySelector(".turn-bars")!);
  expect(view.queryByRole("tooltip")).toBeNull();
});
