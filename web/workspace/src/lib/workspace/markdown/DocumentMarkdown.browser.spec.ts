// @vitest-environment happy-dom

import { cleanup, render, waitFor } from "@testing-library/svelte";
import { afterEach, expect, test } from "vitest";
import DocumentMarkdown from "./DocumentMarkdown.svelte";

afterEach(cleanup);

test("renders document Markdown semantics through the shared safe policy", async () => {
  const source = [
    "# Memory heading",
    "",
    "##### Deep heading",
    "",
    "###### Deepest heading",
    "",
    "A paragraph with **strong context** and `inline-code`.",
    "",
    "- first item",
    "- second item",
    "",
    "> durable quote",
    "",
    "| Decision | Owner |",
    "| --- | --- |",
    "| Keep the document safe | Workspace |",
    "",
    "```rust",
    'fn main() { println!("safe"); }',
    "```",
    "",
    "[safe](https://example.com) [script](javascript:alert(1)) [relative](/internal)",
    "",
    '<img src="x" onerror="alert(1)"><script>alert(1)</script>',
  ].join("\n");
  const { container } = render(DocumentMarkdown, { text: source });

  await waitFor(() => {
    expect(container.querySelector("h1")?.textContent).toBe("Memory heading");
    expect(container.querySelector("table")).not.toBeNull();
  });

  expect(container.querySelector("h5")?.textContent).toBe("Deep heading");
  expect(container.querySelector("h6")?.textContent).toBe("Deepest heading");
  expect(container.querySelector("strong")?.textContent).toBe("strong context");
  expect(container.querySelectorAll("li")).toHaveLength(2);
  expect(container.querySelector("blockquote")?.textContent).toContain(
    "durable quote",
  );
  expect(container.querySelector('a[href="https://example.com"]')).not
    .toBeNull();
  expect(container.querySelectorAll("a")).toHaveLength(1);
  expect(container.querySelector("img")).toBeNull();
  expect(container.querySelector("script")).toBeNull();

  const tableRegion = container.querySelector<HTMLElement>(
    '[role="region"][aria-label="Markdown table"]',
  );
  const codeRegion = container.querySelector<HTMLElement>(
    '[role="region"][aria-label="rust code block"]',
  );
  expect(tableRegion?.tabIndex).toBe(0);
  expect(codeRegion?.tabIndex).toBe(0);
  expect(codeRegion?.querySelector("pre > code")?.textContent).toContain(
    "println!",
  );
});

test("wraps long prose and identifiers without turning them into document-level controls", async () => {
  const longIdentifier = "memory-identifier-".repeat(40);
  const { container } = render(DocumentMarkdown, {
    text:
      `Long value: ${longIdentifier}\n\n<details open><summary>unsafe HTML</summary></details>`,
  });

  await waitFor(() => expect(container.textContent).toContain(longIdentifier));
  expect(container.querySelector("details")).toBeNull();
  expect(container.querySelector("button")).toBeNull();
});
