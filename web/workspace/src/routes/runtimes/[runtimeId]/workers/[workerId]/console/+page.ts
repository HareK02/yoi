export function load(
  { params }: { params: { runtimeId: string; workerId: string } },
) {
  return {
    runtimeId: params.runtimeId,
    workerId: params.workerId,
  };
}
