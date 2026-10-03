// @vitest-environment happy-dom

import { cleanup, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, expect, test } from "vitest";
import SubjectIndexPage from "./+page.svelte";
import SubjectPage from "./[subjectId]/+page.svelte";
import MemoryDetailPage from "./[subjectId]/[memoryId]/+page.svelte";

afterEach(cleanup);

const result = <T>(data: T | null, error: string | null = null) => ({
  data,
  error,
});

function subject() {
  return {
    id: "subject-1",
    role: "Release coordinator",
    state: "active" as const,
    store_revision: 9,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-02T03:04:05Z",
  };
}

test("renders the subject index as the Memory product entry", () => {
  render(SubjectIndexPage, {
    data: {
      workspaceId: "workspace-1",
      cursor: "subject-current",
      subjects: result({
        limit: 100,
        items: [subject()],
        next_cursor: "subject-next",
        has_more: true,
      }),
    },
  } as never);

  expect(screen.getByRole("heading", { name: "Subjects", level: 1 })).not
    .toBeNull();
  expect(screen.getByRole("link", { name: /Release coordinator/ })).not
    .toBeNull();
  expect(screen.getByText("subject-1")).not.toBeNull();
  expect(screen.getByRole("link", { name: "First page" }).getAttribute("href"))
    .toBe("/w/workspace-1/memory");
  expect(screen.getByRole("link", { name: /Next page/ }).getAttribute("href"))
    .toContain("cursor=subject-next");
  expect(screen.queryByText("Staging")).toBeNull();
});

test("renders a ready resident surface and committed Memory lifecycle states", async () => {
  render(SubjectPage, {
    data: {
      workspaceId: "workspace-1",
      subjectId: "subject-1",
      cursor: "memory-current",
      subject: result(subject()),
      surface: result({
        subject_id: "subject-1",
        availability: "ready" as const,
        snapshot: {
          snapshot_id: "snapshot-1",
          body_md: "# Resident context\n\nUse the **current** committed record.",
          memory_refs: [{ memory_id: "memory-1", revision: 3 }],
          built_from_store_revision: 9,
          created_at: "2026-01-02T03:04:05Z",
        },
      }),
      memories: result({
        items: [
          {
            id: "memory-1",
            revision: 3,
            kind: "decision" as const,
            state: "active" as const,
            claim: "Current decision",
            excerpt: "Current excerpt",
            updated_at: "2026-01-02T03:04:05Z",
          },
          {
            id: "memory-2",
            revision: 2,
            kind: "lesson" as const,
            state: "resolved" as const,
            claim: "Resolved lesson",
            excerpt: "Resolved excerpt",
            updated_at: "2026-01-02T03:04:05Z",
          },
          {
            id: "memory-3",
            revision: 1,
            kind: "constraint" as const,
            state: "retracted" as const,
            claim: "Retracted constraint",
            excerpt: "Retracted excerpt",
            updated_at: "2026-01-02T03:04:05Z",
          },
        ],
        next_cursor: "memory-next",
        has_more: true,
      }),
    },
  } as never);

  await waitFor(() => {
    expect(screen.getByRole("heading", { name: "Resident context", level: 1 }))
      .not.toBeNull();
  });
  expect(screen.getByRole("article", { name: "Resident surface" })).not
    .toBeNull();
  expect(screen.getAllByText("active", { exact: true }).length).toBeGreaterThan(
    0,
  );
  expect(screen.getByText("Resolved", { exact: true })).not.toBeNull();
  expect(screen.getByText("Retracted", { exact: true })).not.toBeNull();
  expect(screen.getByRole("navigation", { name: "Current Memory pages" }))
    .not.toBeNull();
  expect(screen.getByRole("link", { name: /Next page/ }).getAttribute("href"))
    .toContain("cursor=memory-next");
});

test("distinguishes ready-empty, stale, failed, unavailable, and request error surfaces", async () => {
  const base = {
    workspaceId: "workspace-1",
    subjectId: "subject-1",
    subject: result(subject()),
    memories: result({ items: [], has_more: false }),
  };
  const view = render(SubjectPage, {
    data: {
      ...base,
      surface: result({
        subject_id: "subject-1",
        availability: "ready" as const,
        snapshot: {
          snapshot_id: "snapshot-empty",
          body_md: "",
          memory_refs: [],
          built_from_store_revision: 9,
          created_at: "2026-01-02T03:04:05Z",
        },
      }),
    },
  } as never);
  expect(screen.getByText("Resident surface is ready and empty.")).not
    .toBeNull();

  await view.rerender(
    {
      data: {
        ...base,
        surface: result({
          subject_id: "subject-1",
          availability: "stale" as const,
        }),
      },
    } as never,
  );
  expect(screen.getByText("Resident surface is stale.")).not.toBeNull();

  await view.rerender(
    {
      data: {
        ...base,
        surface: result({
          subject_id: "subject-1",
          availability: "failed" as const,
        }),
      },
    } as never,
  );
  expect(screen.getByRole("alert").textContent).toContain("generation failed");

  await view.rerender({ data: { ...base, surface: result(null) } } as never);
  expect(screen.getByText("Resident surface data is unavailable.")).not
    .toBeNull();

  await view.rerender(
    {
      data: { ...base, surface: result(null, "Memory API request failed") },
    } as never,
  );
  expect(screen.getByRole("alert").textContent).toContain(
    "Resident surface unavailable",
  );
});

test("renders committed Memory detail, provenance, derivation, and immutable revisions", async () => {
  render(MemoryDetailPage, {
    data: {
      workspaceId: "workspace-1",
      subjectId: "subject-1",
      memoryId: "memory-1",
      revisionCursor: "revision-current",
      subject: result(subject()),
      memory: result({
        memory_id: "memory-1",
        revision: 2,
        current_revision: 3,
        kind: "decision" as const,
        state: "resolved" as const,
        claim: "Keep provenance typed",
        body_md: "# Committed detail\n\nRead-only body.",
        why_useful: "Prevents origin loss.",
        change_reason: "Resolved after implementation.",
        created_at: "2026-01-01T00:00:00Z",
        updated_at: "2026-01-02T03:04:05Z",
        body_offset: 0,
        body_byte_offset: 0,
        body_truncated: false,
        source_candidate_ids: ["candidate-1"],
        source_candidates: [{
          candidate_id: "candidate-1",
          evidence: [{
            id: "evidence-1",
            kind: "message",
            summary: "Explicit decision.",
          }],
          evidence_total: 1,
          evidence_truncated: false,
          source_refs: [{
            session_id: "session-1",
            segment_id: "segment-1",
            evidence_id: "evidence-1",
            label: "Decision discussion",
          }],
          source_refs_total: 1,
          source_refs_truncated: false,
        }],
        derived_from: [{ memory_id: "memory-parent", revision: 4 }],
        evidence_has_more: false,
      }),
      revisions: result({
        memory_id: "memory-1",
        current_revision: 3,
        items: [{
          revision: 3,
          kind: "decision" as const,
          state: "active" as const,
          claim: "Current claim",
          change_reason: "Clarified",
          updated_at: "2026-01-03T00:00:00Z",
        }, {
          revision: 2,
          kind: "decision" as const,
          state: "resolved" as const,
          claim: "Earlier claim",
          change_reason: "Resolved",
          updated_at: "2026-01-02T00:00:00Z",
        }],
        next_cursor: "revision-next",
        has_more: true,
      }),
    },
  } as never);

  await waitFor(() => {
    expect(screen.getByRole("heading", { name: "Committed detail", level: 1 }))
      .not.toBeNull();
  });
  expect(screen.getByText("candidate-1")).not.toBeNull();
  expect(screen.getByText("Decision discussion")).not.toBeNull();
  expect(screen.getByText("memory-parent")).not.toBeNull();
  expect(screen.getByRole("heading", { name: "Revision history" })).not
    .toBeNull();
  expect(screen.getByText("Historical revision")).not.toBeNull();
  expect(screen.getByRole("navigation", { name: "Revision history pages" }))
    .not.toBeNull();
  expect(screen.getByRole("link", { name: /Next page/ }).getAttribute("href"))
    .toContain("revision_cursor=revision-next");
});
