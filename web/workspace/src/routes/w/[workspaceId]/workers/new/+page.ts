export function load(
  { params, url }: { params: { workspaceId: string }; url: URL },
) {
  const ticketId = url.searchParams.get("ticketId") ?? "";
  return {
    workspaceId: params.workspaceId,
    ticketContext: ticketId
      ? {
        ticketId,
        ticketTitle: url.searchParams.get("ticketTitle") ?? ticketId,
        initialInput: url.searchParams.get("initialInput") ??
          `Work on Ticket ${ticketId}.`,
      }
      : null,
  };
}
