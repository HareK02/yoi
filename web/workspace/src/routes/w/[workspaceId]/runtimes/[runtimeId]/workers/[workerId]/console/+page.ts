export function load(
  { params }: {
    params: { workspaceId: string; runtimeId: string; workerId: string };
  },
) {
  return {
    workspaceId: params.workspaceId,
    runtimeId: params.runtimeId,
    workerId: params.workerId,
  };
}
