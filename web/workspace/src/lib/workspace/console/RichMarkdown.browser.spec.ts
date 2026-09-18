// @vitest-environment happy-dom

import { cleanup, render, waitFor } from "@testing-library/svelte";
import { afterEach, expect, test } from "vitest";
import RichMarkdown from "./RichMarkdown.svelte";

afterEach(cleanup);

test("preserves safe links, blocks raw HTML, and renders Shiki code", async () => {
  const source = [
    "[unsafe](javascript:alert(1)) and [safe](https://example.com)",
    "",
    '<img src="x" onerror="alert(1)">',
    "",
    "```ts",
    "const answer: number = 42;",
    "```",
  ].join("\n");
  const { container } = render(RichMarkdown, {
    text: source,
    streamId: "message-security",
  });

  await waitFor(() => {
    expect(container.querySelectorAll("a")).toHaveLength(2);
    expect(container.querySelector("pre.shiki code span")).not.toBeNull();
  });

  const [unsafe, safe] = container.querySelectorAll("a");
  expect(unsafe.getAttribute("href")).toBe("");
  expect(safe.getAttribute("href")).toBe("https://example.com");
  expect(safe.getAttribute("target")).toBe("_blank");
  expect(safe.getAttribute("rel")).toBe("noreferrer");
  expect(container.querySelector("img")).toBeNull();
  expect(container.querySelector("script")).toBeNull();
});

test("keeps completed DOM nodes while a later markdown block grows", async () => {
  const initial = [
    "# Stable heading",
    "",
    "This paragraph is already complete.",
    "",
    "The final paragraph is stream",
  ].join("\n");
  const { container, rerender } = render(RichMarkdown, {
    text: initial,
    streamId: "message-1",
  });

  await waitFor(() => {
    expect(container.querySelector("h1")?.textContent).toBe("Stable heading");
    expect(container.querySelectorAll("p")).toHaveLength(2);
  });
  const heading = container.querySelector("h1");
  const completedParagraph = container.querySelector("p");

  await rerender({
    text: `${initial}ed incrementally.`,
    streamId: "message-1",
  });
  await waitFor(() => {
    expect(container.querySelectorAll("p")[1]?.textContent).toBe(
      "The final paragraph is streamed incrementally.",
    );
  });

  expect(container.querySelector("h1")).toBe(heading);
  expect(container.querySelector("p")).toBe(completedParagraph);
});
