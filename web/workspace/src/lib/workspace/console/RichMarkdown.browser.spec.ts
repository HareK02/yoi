// @vitest-environment happy-dom

import { cleanup, render, waitFor } from "@testing-library/svelte";
import { afterEach, expect, test } from "vitest";
import RichMarkdown from "./RichMarkdown.svelte";

afterEach(cleanup);

test("keeps completed block nodes while a trailing inline token grows", async () => {
  const initial = [
    "# Stable heading",
    "",
    "This paragraph is already complete.",
    "",
    "- stable list item",
    "- another item",
    "",
    "> stable quote",
    "",
    "```ts",
    "const answer: number = 42;",
    "```",
    "",
    "The final paragraph has **partial",
  ].join("\n");
  const { container, rerender } = render(RichMarkdown, {
    text: initial,
    streamId: "message-1",
  });

  await waitFor(() => {
    expect(container.querySelector("h1")?.textContent).toBe("Stable heading");
    expect(container.querySelector("ul")).not.toBeNull();
    expect(container.querySelector("blockquote")).not.toBeNull();
    expect(container.querySelector("pre.shiki code span")).not.toBeNull();
  });
  const stableNodes = [
    container.querySelector("h1"),
    container.querySelector("p"),
    container.querySelector("ul"),
    container.querySelector("blockquote"),
    container.querySelector("pre"),
  ];

  await rerender({
    text: `${initial} emphasis** and a [safe link](https://exam`,
    streamId: "message-1",
  });
  await waitFor(() => {
    expect(container.querySelector("strong")?.textContent).toBe(
      "partial emphasis",
    );
    expect(container.textContent).toContain("[safe link](https://exam");
  });
  expect(container.querySelector('a[href="https://example.com"]')).toBeNull();
  expect(container.querySelector("h1")).toBe(stableNodes[0]);
  expect(container.querySelector("p")).toBe(stableNodes[1]);
  expect(container.querySelector("ul")).toBe(stableNodes[2]);
  expect(container.querySelector("blockquote")).toBe(stableNodes[3]);
  expect(container.querySelector("pre")).toBe(stableNodes[4]);

  await rerender({
    text: `${initial} emphasis** and a [safe link](https://example.com).`,
    streamId: "message-1",
  });
  await waitFor(() => {
    expect(container.querySelector("strong")?.textContent).toBe(
      "partial emphasis",
    );
    expect(container.querySelector('a[href="https://example.com"]')).not
      .toBeNull();
  });

  expect(container.querySelector("h1")).toBe(stableNodes[0]);
  expect(container.querySelector("p")).toBe(stableNodes[1]);
  expect(container.querySelector("ul")).toBe(stableNodes[2]);
  expect(container.querySelector("blockquote")).toBe(stableNodes[3]);
  expect(container.querySelector("pre")).toBe(stableNodes[4]);
});

test("resets the DOM for non-prefix rewrites and stream identity changes", async () => {
  const { container, rerender } = render(RichMarkdown, {
    text: "# Original\n\nOriginal body.",
    streamId: "message-1",
  });
  await waitFor(() => {
    expect(container.querySelector("h1")?.textContent).toBe("Original");
  });
  await rerender({
    text: "# Corrected\n\nRewritten body.",
    streamId: "message-1",
  });
  await waitFor(() => {
    expect(container.querySelector("h1")?.textContent).toBe("Corrected");
  });
  expect(container.textContent).not.toContain("Original body.");

  await rerender({
    text: "# Corrected\n\nRewritten body for a new message.",
    streamId: "message-2",
  });
  await waitFor(() => {
    expect(container.querySelector("p")?.textContent).toBe(
      "Rewritten body for a new message.",
    );
  });
  expect(container.querySelector("h1")?.textContent).toBe("Corrected");
  expect(container.textContent).not.toContain("Original body.");
});

test("preserves line breaks and the HTTP(S)-only link boundary", async () => {
  const source = [
    "first line\nsecond line",
    "",
    "[https](https://example.com) [http](http://example.com)",
    "[script](javascript:alert(1)) [data](data:text/html,hello)",
    "[mail](mailto:test@example.com) [relative](/internal)",
    "",
    '<img src="x" onerror="alert(1)">',
  ].join("\n");
  const { container } = render(RichMarkdown, {
    text: source,
    streamId: "message-security",
  });

  await waitFor(() => {
    expect(container.querySelectorAll("a")).toHaveLength(2);
    expect(container.querySelector("br")).not.toBeNull();
  });

  const links = container.querySelectorAll("a");
  expect(links[0].getAttribute("href")).toBe("https://example.com");
  expect(links[1].getAttribute("href")).toBe("http://example.com");
  expect(links[0].getAttribute("target")).toBe("_blank");
  expect(links[0].getAttribute("rel")).toBe("noreferrer");
  expect(container.textContent).toContain("script");
  expect(container.textContent).toContain("relative");
  expect(container.querySelector("img")).toBeNull();
  expect(container.querySelector("script")).toBeNull();
});

test("keeps prior content through an unclosed fence and unknown-language fallback", async () => {
  const initial = [
    "Stable paragraph.",
    "",
    "```not-a-real-language",
    "raw <text>",
  ].join("\n");
  const { container, rerender } = render(RichMarkdown, {
    text: initial,
    streamId: "message-fence",
  });
  await waitFor(() => {
    expect(container.querySelector("pre.shiki-fallback code")?.textContent)
      .toContain(
        "raw <text>",
      );
  });
  const stableParagraph = container.querySelector("p");

  await rerender({
    text: `${initial}\n\`\`\`\n\nAfter the fence.`,
    streamId: "message-fence",
  });
  await waitFor(() => {
    expect(container.querySelectorAll("p")).toHaveLength(2);
    expect(container.querySelectorAll("p")[1]?.textContent).toBe(
      "After the fence.",
    );
  });
  expect(container.querySelector("p")).toBe(stableParagraph);
  expect(container.querySelector("pre.shiki-fallback code")?.textContent)
    .toContain(
      "raw <text>",
    );
});

test("reuses early DOM nodes across a long streaming append", async () => {
  const firstBatch = Array.from(
    { length: 80 },
    (_, index) => `Paragraph ${index}.`,
  );
  const { container, rerender } = render(RichMarkdown, {
    text: firstBatch.join("\n\n"),
    streamId: "message-long",
  });
  await waitFor(
    () => expect(container.querySelectorAll("p")).toHaveLength(80),
    { timeout: 5_000 },
  );
  const first = container.querySelectorAll("p")[0];
  const middle = container.querySelectorAll("p")[40];

  const secondBatch = Array.from(
    { length: 40 },
    (_, index) => `Paragraph ${index + 80}.`,
  );
  await rerender({
    text: [...firstBatch, ...secondBatch].join("\n\n"),
    streamId: "message-long",
  });
  await waitFor(
    () => expect(container.querySelectorAll("p")).toHaveLength(120),
    { timeout: 5_000 },
  );

  expect(container.querySelectorAll("p")[0]).toBe(first);
  expect(container.querySelectorAll("p")[40]).toBe(middle);
}, 10_000);
