declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
  readTextFile(path: string | URL): Promise<string>;
};

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
    "../../../routes/w/[workspaceId]/tickets/[ticketId]/+page.svelte",
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
