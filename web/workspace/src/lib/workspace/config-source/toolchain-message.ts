export function jsonWorkerMessage<T>(request: T): T {
  return JSON.parse(JSON.stringify(request)) as T;
}
