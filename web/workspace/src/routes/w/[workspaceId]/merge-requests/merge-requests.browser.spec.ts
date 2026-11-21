// @vitest-environment happy-dom
import { cleanup, render, screen } from "@testing-library/svelte";
import { afterEach, expect, test } from "vitest";
import List from "./PageContent.svelte";
import Detail from "./[mergeRequestId]/PageContent.svelte";
import { load as listLoad } from "./+page.ts";
import { load as detailLoad } from "./[mergeRequestId]/+page.ts";
import {
  mergeRequestDetailFixture,
  mergeRequestFindingsFixture,
  mergeRequestListFixture,
} from "#lib/workspace/merge-request.test-fixtures.ts";

afterEach(cleanup);

test("list loader sends URL conditions to the existing API before decoding a bounded page", async () => {
  const requested: string[] = [];
  const event = {
    params: { workspaceId: "tickets" },
    parent: async () => ({}),
    url: new URL(
      "http://localhost/w/tickets/merge-requests?repository_key=main&selector_to=refs%2Fheads%2Fdevelop&phase=integration&cursor=page-2&limit=20",
    ),
    fetch: async (input: string) => {
      requested.push(input);
      return Response.json({
        items: [mergeRequestListFixture()],
        next_cursor: "page-3",
      });
    },
  } as unknown as Parameters<typeof listLoad>[0];
  const result = await listLoad(event).deferredPage;
  expect(result.ok).toBe(true);
  expect(requested).toHaveLength(1);
  const url = new URL(requested[0], event.url);
  expect(url.pathname).toBe("/api/w/tickets/merge-requests");
  expect(url.searchParams.get("selector_to")).toBe("refs/heads/develop");
  expect(url.searchParams.get("phase")).toBe("integration");
  expect(url.searchParams.get("cursor")).toBe("page-2");
  if (result.ok) expect(result.data.mergeRequests?.next_cursor).toBe("page-3");
});

test("detail loader preserves the history cursor and Backend status with no mutation", async () => {
  const requested: string[] = [];
  const event = {
    params: { workspaceId: "tickets", mergeRequestId: "MR-1" },
    parent: async () => ({}),
    url: new URL("http://localhost/w/tickets/merge-requests/MR-1?after=50"),
    fetch: async (input: string) => {
      requested.push(input);
      return Response.json(mergeRequestDetailFixture());
    },
  } as unknown as Parameters<typeof detailLoad>[0];
  const result = await detailLoad(event).deferredPage;
  expect(result.ok).toBe(true);
  expect(requested).toEqual([
    "/api/w/tickets/merge-requests/MR-1?limit=100&after=50",
  ]);
  if (result.ok) {
    expect(result.data.after).toBe("50");
    expect(result.data.mergeRequest?.summary.review_status).toBe(
      "request_changes",
    );
    expect(result.data.mergeRequest?.thread[0].kind).toBe("comment");
  }
});
const listData = () => ({
  workspaceId: "tickets",
  query:
    "repository_key=main&selector_to=develop&phase=integration&limit=20&cursor=page-2",
  error: null,
  mergeRequests: {
    items: [mergeRequestListFixture()],
    next_cursor: "page-3" as string | null,
  },
});
const detailData = () => ({
  workspaceId: "tickets",
  mergeRequestId: "MR-1",
  after: null,
  error: null,
  mergeRequest: mergeRequestDetailFixture(),
});

test("list preserves query and paging while request changes remains Integration", () => {
  render(List, { data: listData() } as never);
  expect((screen.getByLabelText("Repository") as HTMLInputElement).value).toBe(
    "main",
  );
  expect((screen.getByLabelText("Target") as HTMLInputElement).value).toBe(
    "develop",
  );
  expect((screen.getByLabelText("Phase") as HTMLSelectElement).value).toBe(
    "integration",
  );
  expect(screen.getByText("Phase: integration")).toBeTruthy();
  expect(screen.getByText(/Live source review: request_changes/)).toBeTruthy();
  const next = new URL(
    screen.getByRole("link", { name: "Next page" }).getAttribute("href")!,
    "http://localhost",
  );
  expect(next.searchParams.get("cursor")).toBe("page-3");
  expect(next.searchParams.get("selector_to")).toBe("develop");
  expect(next.searchParams.get("phase")).toBe("integration");
  expect(next.searchParams.get("limit")).toBe("20");
  const first = screen.getByRole("link", { name: "First page" }).getAttribute(
    "href",
  )!;
  expect(first).not.toContain("cursor");
  const form = screen.getByRole("search");
  expect(form.querySelector('[name="cursor"]')).toBeNull();
  expect(form.querySelector('[name="assigned_to_self"]')).toBeNull();
});

test("empty and permission error retain conditions without reporting stale rows", async () => {
  const data = listData();
  data.mergeRequests.items = [];
  data.mergeRequests.next_cursor = null;
  const page = render(List, { data } as never);
  expect(screen.getByText("No Merge Requests match these conditions."))
    .toBeTruthy();
  expect(screen.queryByRole("link", { name: "Next page" })).toBeNull();
  await page.rerender({
    data: { ...data, error: "Permission denied", mergeRequests: null },
  } as never);
  expect(screen.getByRole("alert").textContent).toBe("Permission denied");
  expect((screen.getByLabelText("Target") as HTMLInputElement).value).toBe(
    "develop",
  );
  expect(screen.queryByText("No Merge Requests match these conditions."))
    .toBeNull();
});

test("detail shows Backend comments without deriving completion or exposing a receipt surface", () => {
  const data = detailData();
  data.mergeRequest.event_page = {
    ...data.mergeRequest.event_page,
    has_more: true,
    next_after: 50,
  };
  render(Detail, { data } as never);
  expect(screen.getByText(/Publication outcome is unknown/)).toBeTruthy();
  expect(screen.getByText("target_unknown")).toBeTruthy();
  expect(screen.getByText(/Provider observation unavailable/)).toBeTruthy();
  expect(screen.getByText(/Current source requests changes/)).toBeTruthy();
  expect(screen.getByText("integration", { exact: true })).toBeTruthy();
  expect(screen.getByRole("link", { name: "Next events" }).getAttribute("href"))
    .toBe("/w/tickets/merge-requests/MR-1?after=50");
  expect(screen.queryByRole("heading", { name: "Publication operations" }))
    .toBeNull();
  expect(screen.queryByRole("button", { name: /publish|complete|retry/i }))
    .toBeNull();
});

test("released assignment and terminal phase are not inferred from historical worker or publication", () => {
  const data = detailData();
  data.mergeRequest.worker_assignment!.worker = null;
  data.mergeRequest.state = {
    kind: "canceled",
    from: { kind: "integration", started_by: "approval-1" },
  };
  render(Detail, { data } as never);
  expect(screen.getByText("Unassigned")).toBeTruthy();
  expect(screen.queryByText("Worker identity")).toBeNull();
  expect(screen.queryByText("Backend readiness evaluation")).toBeNull();
  expect(
    screen.getByText(
      "Merge Request canceled; cancellation is not evidence of target publication.",
    ),
  ).toBeTruthy();
  expect(screen.getByText(/Publication outcome is unknown/)).toBeTruthy();
});

test("findings-only reviews show all typed findings and retain history navigation", async () => {
  const data = { ...detailData(), mergeRequest: mergeRequestFindingsFixture() };
  const page = render(Detail, { data } as never);
  const review = data.mergeRequest.thread[0];
  if (review.kind !== "review") throw new Error("review fixture required");
  expect(screen.getByRole("list", { name: "Review findings" }).children)
    .toHaveLength(4);
  for (const finding of review.findings) {
    expect(
      screen.getByText(finding.body, { exact: true, normalizer: (s) => s }),
    ).toBeTruthy();
    expect(screen.getByText(finding.severity, { exact: true })).toBeTruthy();
  }
  expect(screen.getByText("publication_rejection")).toBeTruthy();
  expect(
    screen.getByText(
      "crates/worker-runtime/src/working_directory/publication.rs",
    ),
  ).toBeTruthy();
  expect(screen.getByText("Line 350")).toBeTruthy();
  expect(screen.getByText("Line 12")).toBeTruthy();
  const findings =
    screen.getByRole("list", { name: "Review findings" }).children;
  expect(findings[1].querySelector(".mr-finding-location")?.textContent?.trim())
    .toBe("major");
  expect(findings[2].querySelector(".mr-finding-location span")).toBeNull();
  expect(findings[3].querySelector("code")).toBeNull();
  expect(page.container.querySelector(".mr-event-body")).toBeNull();
  expect(screen.getByRole("link", { name: "Next events" }).getAttribute("href"))
    .toBe("/w/tickets/merge-requests/MR-1?after=50");
  await page.rerender({
    data: {
      ...data,
      after: "50",
      mergeRequest: {
        ...data.mergeRequest,
        thread: [{ ...review, sequence: 51 }],
        event_page: {
          ...data.mergeRequest.event_page,
          has_more: false,
          next_after: null,
        },
      },
    },
  } as never);
  expect(screen.getByRole("link", { name: "First events" })).toBeTruthy();
  expect(screen.queryByRole("link", { name: "Next events" })).toBeNull();
  expect(screen.getByRole("list", { name: "Review findings" }).children)
    .toHaveLength(4);
});
