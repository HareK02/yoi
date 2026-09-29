// @vitest-environment happy-dom

import { cleanup, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, expect, test } from "vitest";
import MemoryPage from "./+page.svelte";
import type { PageProps } from "./$types";

function pageProps(memory: {
  data: {
    body_md: string;
    created_at: string;
    updated_at: string;
    bytes: number;
    record_source: string;
  } | null;
  error: string | null;
}): PageProps {
  return {
    data: { workspaceId: "workspace-1", memory },
    params: { workspaceId: "workspace-1" },
  } as unknown as PageProps;
}

afterEach(cleanup);

test("makes the Markdown document primary and keeps technical metadata in disclosure", async () => {
  const { container } = render(
    MemoryPage,
    pageProps({
      data: {
        body_md: "# Durable context\n\nRead the **current** document.",
        created_at: "2026-01-01T00:00:00Z",
        updated_at: "2026-01-02T03:04:05Z",
        bytes: 48,
        record_source: "sqlite_workspace_authority.memory_document",
      },
      error: null,
    }),
  );

  await waitFor(() => {
    expect(screen.getByRole("heading", { name: "Durable context", level: 1 })).not.toBeNull();
  });
  expect(screen.getByText("Read-only")).not.toBeNull();
  expect(screen.getByText("Document details")).not.toBeNull();
  expect(container.querySelector(".memory-document-page")?.classList.contains("card")).toBe(false);
  expect(screen.queryByRole("heading", { name: "Memory Document" })).toBeNull();
  expect(screen.getByRole("article", { name: "Memory document content" })).not.toBeNull();
});

test("distinguishes an empty document from request failure and unavailable data", async () => {
  const view = render(
    MemoryPage,
    pageProps({
      data: {
        body_md: "  \n",
        created_at: "2026-01-01T00:00:00Z",
        updated_at: "2026-01-02T03:04:05Z",
        bytes: 3,
        record_source: "fixture",
      },
      error: null,
    }),
  );
  expect(screen.getByRole("status").textContent).toContain("Memory document is empty.");
  expect(screen.queryByRole("alert")).toBeNull();

  await view.rerender(pageProps({ data: null, error: "Memory API request failed" }));
  expect(screen.getByRole("alert").textContent).toContain("Memory document unavailable.");
  expect(screen.getByRole("alert").textContent).toContain("Memory API request failed");
  expect(screen.queryByText("Memory document is empty.")).toBeNull();

  await view.rerender(pageProps({ data: null, error: null }));
  expect(screen.getByRole("status").textContent).toBe("Memory document data is unavailable.");
  expect(screen.queryByRole("alert")).toBeNull();
});
