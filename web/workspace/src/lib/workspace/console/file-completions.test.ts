import { FileCompletions } from "./file-completions.ts";
import {
  deepStrictEqual as assertEquals,
  notEqual,
  rejects,
} from "node:assert/strict";

declare const Deno: { test(name: string, fn: () => Promise<void>): void };

Deno.test("completion nonces fence same prefix/context ABA without draining cancelled requests", async () => {
  const sent: Array<[string, string]> = [];
  const lane = new FileCompletions((prefix, id) => sent.push([prefix, id]));
  const first = new AbortController();
  const a = lane.request("same", first.signal);
  first.abort();
  assertEquals(await a, []);
  const b = lane.request("other", new AbortController().signal);
  const c = lane.request("same", new AbortController().signal);
  assertEquals(await b, []);
  assertEquals(sent.map(([prefix]) => prefix), ["same", "other", "same"]);
  notEqual(sent[0][1], sent[2][1]);
  lane.receive([{ value: "old-a" }], "same", sent[0][1]);
  lane.receive([{ value: "legacy" }], "same");
  lane.receive([{ value: "wrong-prefix" }], "other", sent[2][1]);
  assertEquals(lane.pending, true);
  lane.receive([{ value: "current-c" }], "same", sent[2][1]);
  assertEquals(await c, [{ value: "current-c" }]);
  lane.close();
});

Deno.test("multiple clients cannot consume each other's identical completion broadcasts", async () => {
  const ids: string[] = [];
  const lanes = [
    new FileCompletions((_prefix, id) => ids.push(id)),
    new FileCompletions((_prefix, id) => ids.push(id)),
  ];
  const promises = lanes.map((lane) =>
    lane.request("same", new AbortController().signal)
  );
  notEqual(ids[0], ids[1]);
  for (const lane of lanes) {
    lane.receive([{ value: "client-a" }], "same", ids[0]);
  }
  assertEquals(await promises[0], [{ value: "client-a" }]);
  assertEquals(lanes[1].pending, true);
  for (const lane of lanes) {
    lane.receive([{ value: "client-b" }], "same", ids[1]);
  }
  assertEquals(await promises[1], [{ value: "client-b" }]);
  lanes.forEach((lane) => lane.close());
});

Deno.test("completion timeout and reconnect invalidate old nonces but allow a fresh request", async () => {
  const ids: string[] = [];
  const lane = new FileCompletions((_prefix, id) => ids.push(id), 5);
  await rejects(
    lane.request("same", new AbortController().signal),
    /timed out/,
  );
  lane.reset();
  const current = lane.request("same", new AbortController().signal);
  lane.receive([{ value: "old" }], "same", ids[0]);
  assertEquals(lane.pending, true);
  lane.receive([{ value: "new" }], "same", ids[1]);
  assertEquals(await current, [{ value: "new" }]);
  lane.close();
});

Deno.test("completion close rejects active request and fences requests until reset", async () => {
  const lane = new FileCompletions(() => {});
  const a = lane.request("a", new AbortController().signal);
  const rejected = rejects(a, /disconnected/);
  lane.close(new Error("disconnected"));
  await rejected;
  await rejects(
    lane.request("b", new AbortController().signal),
    /disconnected/,
  );
});
