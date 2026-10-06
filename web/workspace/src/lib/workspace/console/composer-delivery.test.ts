declare const Deno: {
  test(name: string, fn: () => void): void;
};

import { deepStrictEqual, throws } from "node:assert/strict";
import type { Segment } from "#lib/generated/protocol.ts";
import type { ComposerDraftSnapshot } from "./composer-draft.ts";
import {
  canDeliverComposerDraft,
  ComposerAdmissions,
  sendComposerDelivery,
} from "./composer-delivery.ts";

function assertEquals(actual: unknown, expected: unknown): void {
  if (actual !== expected) {
    throw new Error(`Expected ${String(expected)}, got ${String(actual)}`);
  }
}

const upload: Segment = {
  kind: "uploaded_file",
  file: {
    artifact_id: "staged-upload",
    file_name: "資料.md",
    media_type: "text/markdown",
    created_at_ms: 1,
    availability: "available",
    byte_len: 4,
    sha256: "abcd",
  },
};
const typedSnapshot: ComposerDraftSnapshot = {
  document: "exact typed draft",
  content: 'before /run(path="a b/資料") after',
  pastes: [],
  textPastes: [],
  segments: [
    { kind: "text", content: "before " },
    {
      kind: "feature_invoke",
      invocation: {
        invocation_id: "stable-invocation",
        identity: "feature:test/run",
        name: "run",
        arguments: [{
          name: "path",
          value: { kind: "string", value: "a b/資料" },
        }],
      },
    },
    upload,
    { kind: "text", content: " after" },
  ],
};

Deno.test("Submit admission keeps exact typed draft and stable IDs across rejection and retry", () => {
  const admissions = new ComposerAdmissions();
  const method = {
    method: "submit" as const,
    params: {
      submission_request_id: "stable-request",
      input: typedSnapshot.segments,
    },
  };
  const first = admissions.begin("target-a", method, typedSnapshot, "submit");
  deepStrictEqual(first.snapshot, typedSnapshot);
  assertEquals(admissions.protects(upload), true);
  assertEquals(
    admissions.acknowledge("target-b", {
      event: "submission_accepted",
      data: {
        submission_request_id: "stable-request",
        submission_id: "s",
        disposition: "started",
      },
    }),
    null,
  );
  assertEquals(
    admissions.acknowledge("target-a", {
      event: "submission_rejected",
      data: {
        submission_request_id: "other-client",
        message: "unrelated",
      },
    }),
    null,
  );
  const rejection = admissions.acknowledge("target-a", {
    event: "submission_rejected",
    data: {
      submission_request_id: "stable-request",
      message: "validation failed",
    },
  });
  assertEquals(rejection?.accepted, false);
  deepStrictEqual(rejection?.record.snapshot, typedSnapshot);
  assertEquals(admissions.protects(upload), false);
  const retry = admissions.begin(
    "target-a",
    {
      ...method,
      params: { ...method.params, submission_request_id: "do-not-use-new-id" },
    },
    typedSnapshot,
    "submit",
  );
  deepStrictEqual(retry.method, first.method);
  const accepted = admissions.acknowledge("target-a", {
    event: "submission_accepted",
    data: {
      submission_request_id: "stable-request",
      submission_id: "s",
      disposition: "queued",
    },
  });
  assertEquals(accepted?.accepted, true);
  deepStrictEqual(accepted?.record.snapshot.segments, typedSnapshot.segments);
  assertEquals(admissions.get("target-a"), null);
});

Deno.test("unknown admission protects uploads and in-flight retries from draft deletion and target ABA", () => {
  const admissions = new ComposerAdmissions();
  const method = {
    method: "submit" as const,
    params: {
      submission_request_id: "unknown-request",
      input: typedSnapshot.segments,
    },
  };
  admissions.begin("target-a", method, typedSnapshot, "queue");
  admissions.disconnected("target-a");
  assertEquals(admissions.get("target-a")?.status, "unknown");
  assertEquals(admissions.protects(upload), true);
  throws(
    () =>
      admissions.begin(
        "target-a",
        method,
        { ...typedSnapshot, segments: [] },
        "queue",
      ),
    /in-flight/,
  );
  admissions.disconnected("target-b");
  deepStrictEqual(admissions.retry("target-a")?.method, method);
  deepStrictEqual(admissions.get("target-a")?.snapshot, typedSnapshot);
  assertEquals(admissions.protects(upload), true);
});

Deno.test("Notify admission correlates authoritative rejection/success and retries unchanged IDs", () => {
  const admissions = new ComposerAdmissions();
  const snapshot = {
    ...typedSnapshot,
    segments: [{ kind: "text" as const, content: "通知" }],
  };
  const method = {
    method: "notify" as const,
    params: { notification_request_id: "notification-1", message: "通知" },
  };
  admissions.begin("target", method, snapshot, "notify");
  assertEquals(
    admissions.acknowledge("target", {
      event: "notification_rejected",
      data: { notification_request_id: "other", message: "old" },
    }),
    null,
  );
  assertEquals(
    admissions.acknowledge("target", {
      event: "notification_rejected",
      data: {
        notification_request_id: "notification-1",
        message: "not running",
      },
    })?.accepted,
    false,
  );
  deepStrictEqual(
    admissions.begin(
      "target",
      {
        ...method,
        params: { ...method.params, notification_request_id: "new" },
      },
      snapshot,
      "notify",
    ).method,
    method,
  );
  assertEquals(
    admissions.acknowledge("target", {
      event: "notification_accepted",
      data: { notification_request_id: "notification-1" },
    })?.accepted,
    true,
  );
  assertEquals(admissions.get("target"), null);
});

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
