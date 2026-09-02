export function load(
  { params, url }: { params: { workspaceId: string }; url: URL },
) {
  const ticketId = url.searchParams.get("ticketId") ?? "";
  const ticketRole = url.searchParams.get("ticketRole") ?? "";
  return {
    workspaceId: params.workspaceId,
    ticketContext: ticketId
      ? {
        ticketId,
        ticketTitle: url.searchParams.get("ticketTitle") ?? ticketId,
        ticketRole,
        initialInput: url.searchParams.get("initialInput") ?? "",
        repositoryKey: url.searchParams.get("repositoryKey") ?? "",
        refSelector: url.searchParams.get("refSelector") ?? "HEAD",
      }
      : null,
  };
}
