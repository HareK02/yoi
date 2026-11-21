declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
  readTextFile(path: string | URL): Promise<string>;
};

import { mergeRequestDetailFixture } from "../merge-request.test-fixtures.ts";
import { sourceReviewFreshness, targetIntegrationStatus } from "../merge-request-status.ts";

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

const ticketLoader = await Deno.readTextFile(
  new URL(
    "../../../routes/w/[workspaceId]/tickets/[ticketId]/+page.ts",
    import.meta.url,
  ),
);
const ticketPage = await Deno.readTextFile(
  new URL(
    "../../../routes/w/[workspaceId]/tickets/[ticketId]/PageContent.svelte",
    import.meta.url,
  ),
);
const listLoader = await Deno.readTextFile(
  new URL(
    "../../../routes/w/[workspaceId]/merge-requests/+page.ts",
    import.meta.url,
  ),
);
const detailLoader = await Deno.readTextFile(
  new URL(
    "../../../routes/w/[workspaceId]/merge-requests/[mergeRequestId]/+page.ts",
    import.meta.url,
  ),
);
const detailPage = await Deno.readTextFile(
  new URL(
    "../../../routes/w/[workspaceId]/merge-requests/[mergeRequestId]/PageContent.svelte",
    import.meta.url,
  ),
);
const statusProjection = await Deno.readTextFile(
  new URL("../merge-request-status.ts", import.meta.url),
);
const sidebar = await Deno.readTextFile(
  new URL("../sidebar/WorkspaceSidebar.svelte", import.meta.url),
);

Deno.test("Ticket detail links to a first-class Merge Request resource", () => {
  assert(
    !ticketLoader.includes("`${ticketPath}/merge-request`"),
    "Ticket loader still locates MR detail through a Ticket route",
  );
  assert(
    ticketPage.includes(
      "mergeRequestPagePath(data.workspaceId, mergeRequest.merge_request_id)",
    ),
    "Ticket panel does not link to the MR resource identity",
  );
});

Deno.test("Workspace exposes Merge Request collection and detail pages", () => {
  assert(
    listLoader.includes("mergeRequestCollectionPath(params.workspaceId)"),
    "missing MR list API",
  );
  assert(
    detailLoader.includes(
      "mergeRequestDetailPath(params.workspaceId, params.mergeRequestId)",
    ),
    "missing MR detail API",
  );
  assert(
    sidebar.includes("MergeRequestsNavSection"),
    "MR resources are absent from navigation",
  );
});

Deno.test("Merge Request UI separates source review freshness from target integration", () => {
  assert(
    detailPage.includes("sourceReviewFreshness") &&
      detailPage.includes("targetIntegrationStatus"),
    "MR detail page does not render authority status projections",
  );
  assert(
    ticketPage.includes("summarySourceReviewStatus(mergeRequest)"),
    "Ticket page does not render the source observation/integration evidence projection",
  );
  for (
    const source of [
      `${detailPage}\n${statusProjection}`,
      `${ticketPage}\n${statusProjection}`,
    ]
  ) {
    assert(
      source.includes("Fresh source review required"),
      "missing source-review freshness diagnostic",
    );
    assert(
      source.includes("Target integration"),
      "missing target-integration status",
    );
  }
  assert(
    statusProjection.includes("selector_from moved from"),
    "source ref mismatch is not explained",
  );
  const mr = mergeRequestDetailFixture();
  mr.summary.review_status = "approved";
  const approved = sourceReviewFreshness(mr);
  mr.target = { status: "known", ref: "moved-target", observed_at: mr.updated_at };
  assert(sourceReviewFreshness(mr) === approved, "target-only movement invalidated source review");
  assert(targetIntegrationStatus(mr).includes("completion has not been recorded"), "target observation implied completion");
  mr.state = { kind: "completed", completion: { kind: "published", merge_event_id: "merge", started_by: "approval" } };
  assert(targetIntegrationStatus(mr).includes("retained completion evidence"), "saved completion was not distinguished");
  assert(!statusProjection.includes("CompleteMergeRequest"), "removed tool must not be advertised");
});
