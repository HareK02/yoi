declare const Deno: {
  test(name: string, fn: () => void): void;
};

import {
  canDeliverComposerDraft,
  sendComposerDelivery,
} from "./composer-delivery.ts";

function assertEquals(actual: unknown, expected: unknown): void {
  if (actual !== expected) {
    throw new Error(`Expected ${String(expected)}, got ${String(actual)}`);
  }
}

const base = {
  protocolOpen: true,
  sending: false,
  hasText: true,
  hasAttachments: false,
};

Deno.test("running Composer enables Queue Submit and Notify but not immediate Submit", () => {
  assertEquals(
    canDeliverComposerDraft({
      ...base,
      delivery: "queue",
      workerState: "running",
    }),
    true,
  );
  assertEquals(
    canDeliverComposerDraft({
      ...base,
      delivery: "notify",
      workerState: "running",
    }),
    true,
  );
  assertEquals(
    canDeliverComposerDraft({
      ...base,
      delivery: "submit",
      workerState: "running",
    }),
    false,
  );
});

Deno.test("running Queue Submit and Notify dispatch their protocol methods", () => {
  const sent: string[] = [];
  assertEquals(
    sendComposerDelivery(
      { ...base, delivery: "queue", workerState: "running" },
      "submit",
      (method) => sent.push(method),
    ),
    true,
  );
  assertEquals(
    sendComposerDelivery(
      { ...base, delivery: "notify", workerState: "running" },
      "notify",
      (method) => sent.push(method),
    ),
    true,
  );
  assertEquals(sent.join(","), "submit,notify");
});

Deno.test("idle Composer enables only immediate Submit", () => {
  assertEquals(
    canDeliverComposerDraft({
      ...base,
      delivery: "submit",
      workerState: "idle",
    }),
    true,
  );
  assertEquals(
    canDeliverComposerDraft({
      ...base,
      delivery: "queue",
      workerState: "idle",
    }),
    false,
  );
  assertEquals(
    canDeliverComposerDraft({
      ...base,
      delivery: "notify",
      workerState: "idle",
    }),
    false,
  );
});

Deno.test("running delivery remains fenced by protocol, send state, and payload kind", () => {
  assertEquals(
    canDeliverComposerDraft({
      ...base,
      delivery: "queue",
      workerState: "running",
      protocolOpen: false,
    }),
    false,
  );
  assertEquals(
    canDeliverComposerDraft({
      ...base,
      delivery: "notify",
      workerState: "running",
      sending: true,
    }),
    false,
  );
  assertEquals(
    canDeliverComposerDraft({
      ...base,
      delivery: "notify",
      workerState: "running",
      hasAttachments: true,
    }),
    false,
  );
  assertEquals(
    canDeliverComposerDraft({
      ...base,
      delivery: "queue",
      workerState: "running",
      hasText: false,
      hasAttachments: true,
    }),
    true,
  );
});
