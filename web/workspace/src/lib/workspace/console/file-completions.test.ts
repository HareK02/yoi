import { FileCompletions } from "./file-completions.ts";
import { deepStrictEqual as assertEquals, rejects } from "node:assert/strict";

async function assertRejects(
  fn: () => Promise<unknown>,
  _type: typeof Error,
  message: string,
) {
  await rejects(
    fn,
    (error: unknown) =>
      error instanceof Error && error.message.includes(message),
  );
}

declare const Deno: { test(name: string, fn: () => Promise<void>): void };

Deno.test("file completion serializes the latest prefix behind cancelled wire replies", async () => {
  const sent: string[] = [];
  const lane = new FileCompletions((prefix) => sent.push(prefix));
  const first = new AbortController();
  const a = lane.request("a", first.signal);
  first.abort();
  assertEquals(await a, []);
  const second = new AbortController();
  const b = lane.request("b", second.signal);
  second.abort();
  const c = lane.request("c", new AbortController().signal);
  assertEquals(await b, []);
  assertEquals(sent, ["a"]);
  lane.receive([{ value: "old-a" }]);
  assertEquals(sent, ["a", "c"]);
  lane.receive([{ value: "current-c" }]);
  assertEquals(await c, [{ value: "current-c" }]);
  lane.close();
});

Deno.test("file completion timeout blocks late responses until a new connection", async () => {
  const sent: string[] = [];
  const lane = new FileCompletions((prefix) => sent.push(prefix), 5);
  await assertRejects(
    () => lane.request("a", new AbortController().signal),
    Error,
    "timed out",
  );
  lane.receive([{ value: "late-a" }]);
  await assertRejects(
    () => lane.request("b", new AbortController().signal),
    Error,
    "timed out",
  );
  assertEquals(sent, ["a"]);
  lane.reset();
  const c = lane.request("c", new AbortController().signal);
  lane.receive([{ value: "c" }]);
  assertEquals(await c, [{ value: "c" }]);
  lane.close();
});

Deno.test("file completion close rejects both active and queued requests", async () => {
  const lane = new FileCompletions(() => {});
  const a = lane.request("a", new AbortController().signal);
  const b = lane.request("b", new AbortController().signal);
  const outcomes = Promise.allSettled([a, b]);
  lane.close(new Error("disconnected"));
  assertEquals((await outcomes).map((value) => value.status), [
    "rejected",
    "rejected",
  ]);
});
