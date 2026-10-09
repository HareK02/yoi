import type { DriveEntry, DriveEntryRef } from "./api.ts";
export function assert(
  condition: unknown,
  message = "Assertion failed",
): asserts condition {
  if (!condition) throw new Error(message);
}
export function equal(actual: unknown, expected: unknown): void {
  assert(
    JSON.stringify(actual) === JSON.stringify(expected),
    `Expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
  );
}
export function throws(fn: () => unknown): void {
  try {
    fn();
  } catch {
    return;
  }
  throw new Error("Expected rejection");
}
export async function rejects(
  fn: () => Promise<unknown>,
  check?: (error: unknown) => void,
): Promise<void> {
  try {
    await fn();
  } catch (error) {
    check?.(error);
    return;
  }
  throw new Error("Expected rejection");
}
export function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
export function ref(id = "2", ws = "alpha"): DriveEntryRef {
  return { workspace_id: ws, node_id: id };
}
export function entry(
  id = "2",
  ws = "alpha",
  revision = "9007199254740993",
  kind: "file" | "folder" = "file",
): DriveEntry {
  return {
    entry: ref(id, ws),
    parent: id === "1" ? null : ref("1", ws),
    name: id === "1" ? "" : "note.txt",
    kind,
    revision,
    size: kind === "file" ? 3 : null,
    content_type: kind === "file" ? "text/plain" : null,
    updated_by: "user:test",
    updated_at: "2026-10-09T00:00:00Z",
    latest_url: `/api/w/${ws}/drive/${
      kind === "file" ? "download" : "metadata"
    }?entry_workspace_id=${ws}&id=${id}`,
  };
}
