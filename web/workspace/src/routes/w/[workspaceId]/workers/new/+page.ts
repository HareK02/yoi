export function load({ params }: { params: { workspaceId: string } }) {
  return {
    workspaceId: params.workspaceId,
  };
}
