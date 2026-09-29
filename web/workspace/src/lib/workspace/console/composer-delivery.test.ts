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

Deno.test("Queue accepts text and attachments while running or paused", () => {
  for (const workerState of ["running", "paused"]) {
    for (
      const payload of [
        { hasText: true, hasAttachments: false },
        { hasText: false, hasAttachments: true },
        { hasText: true, hasAttachments: true },
      ]
    ) {
      assertEquals(
        canDeliverComposerDraft({
          ...base,
          ...payload,
          workerState,
          delivery: "queue",
        }),
        true,
      );
    }
  }
});

Deno.test("Queue respects lifecycle, transport, send and empty-input fences", () => {
  for (const workerState of ["idle", "stopped", "loading", "unknown"]) {
    assertEquals(
      canDeliverComposerDraft({
        ...base,
        workerState,
        delivery: "queue",
      }),
      false,
    );
  }
  for (
    const fence of [
      { protocolOpen: false },
      { sending: true },
      { hasText: false, hasAttachments: false },
    ]
  ) {
    const state = {
      ...base,
      ...fence,
      workerState: "running",
      delivery: "queue" as const,
    };
    assertEquals(canDeliverComposerDraft(state), false);
    let sent = false;
    assertEquals(
      sendComposerDelivery(state, "submit", () => {
        sent = true;
      }),
      false,
    );
    assertEquals(sent, false);
  }
});

Deno.test("Queue dispatches Submit, not Notify", () => {
  const sent: string[] = [];
  assertEquals(
    sendComposerDelivery(
      { ...base, workerState: "running", delivery: "queue" },
      "submit",
      (method) => sent.push(method),
    ),
    true,
  );
  assertEquals(sent.join(","), "submit");
});

Deno.test("running Composer enables Notify but not Submit", () => {
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

Deno.test("running Notify dispatches its protocol method", () => {
  const sent: string[] = [];
  assertEquals(
    sendComposerDelivery(
      { ...base, delivery: "notify", workerState: "running" },
      "notify",
      (method) => sent.push(method),
    ),
    true,
  );
  assertEquals(sent.join(","), "notify");
});

Deno.test("idle Composer enables only Submit", () => {
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
      delivery: "notify",
      workerState: "idle",
    }),
    false,
  );
});

Deno.test("paused Composer enables neither Submit nor Notify", () => {
  assertEquals(
    canDeliverComposerDraft({
      ...base,
      delivery: "submit",
      workerState: "paused",
    }),
    false,
  );
  assertEquals(
    canDeliverComposerDraft({
      ...base,
      delivery: "notify",
      workerState: "paused",
    }),
    false,
  );
});

Deno.test("running Notify remains fenced by protocol, send state, and payload kind", () => {
  assertEquals(
    canDeliverComposerDraft({
      ...base,
      delivery: "notify",
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
});
