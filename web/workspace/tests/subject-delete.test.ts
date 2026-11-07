import {
  deleteSubjektivSubject,
  parseSubjektivSubjectDeleteResponse,
  SubjektivSubjectDeleteError,
} from "../src/lib/workspace/memory/api.ts";
import {
  clearSubjectDeleteAttempt,
  readSubjectDeleteAttempt,
  saveSubjectDeleteAttempt,
  subjectDeleteStorageKey,
} from "../src/lib/workspace/memory/subject-delete.ts";

declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

function equal(actual: unknown, expected: unknown) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `Expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}
Deno.test("Subject deletion uses DELETE with the same request after fence-only success", async () => {
  const calls: unknown[] = [];
  const fetcher = ((input, init) => {
    calls.push([input, init]);
    return Promise.resolve(
      Response.json({
        subject_id: "s/1",
        operation_id: "same-operation",
        completed: calls.length > 1,
      }),
    );
  }) as typeof fetch;
  for (const completed of [false, true]) {
    equal(
      await deleteSubjektivSubject(fetcher, "w/1", "s/1", {
        operation_id: "same-operation",
      }),
      { subject_id: "s/1", operation_id: "same-operation", completed },
    );
  }
  equal(calls[0], calls[1]);
  equal(calls[0], ["/api/w/w%2F1/subjektiv/subjects/s%2F1", {
    method: "DELETE",
    headers: { "content-type": "application/json" },
    body: '{"operation_id":"same-operation"}',
  }]);
});
Deno.test("Subject deletion never confirms a malformed or wrong-target response", () => {
  for (
    const payload of [
      { subject_id: "other", operation_id: "op", completed: true },
      { subject_id: "s", operation_id: "other", completed: true },
      { subject_id: "s", operation_id: "op", completed: "true" },
      { subject_id: "s", operation_id: "op" },
      { subject_id: "s", operation_id: "op", completed: true, extra: 1 },
    ]
  ) {
    try {
      parseSubjektivSubjectDeleteResponse(payload, "s", "op");
    } catch {
      continue;
    }
    throw new Error("Accepted an unconfirmed response");
  }
});
Deno.test("Subject deletion classifies rejection separately from unknown transport/server/response outcomes", async () => {
  for (
    const item of [
      {
        fetcher: () => Promise.reject(new Error("lost")),
        kind: "unknown_outcome",
      },
      {
        fetcher: () =>
          Promise.resolve(
            Response.json({ error: "forbidden" }, { status: 403 }),
          ),
        kind: "rejected",
      },
      {
        fetcher: () =>
          Promise.resolve(
            Response.json({ error: "conflict" }, { status: 409 }),
          ),
        kind: "rejected",
      },
      {
        fetcher: () =>
          Promise.resolve(
            Response.json({ error: "uncertain" }, { status: 500 }),
          ),
        kind: "unknown_outcome",
      },
      {
        fetcher: () =>
          Promise.resolve(
            Response.json({
              subject_id: "wrong",
              operation_id: "op",
              completed: true,
            }),
          ),
        kind: "unknown_outcome",
      },
    ]
  ) {
    try {
      await deleteSubjektivSubject(item.fetcher as typeof fetch, "w", "s", {
        operation_id: "op",
      });
    } catch (error) {
      if (!(error instanceof SubjektivSubjectDeleteError)) throw error;
      equal(error.kind, item.kind);
      continue;
    }
    throw new Error("Unexpected success");
  }
});
Deno.test("Retry input survives reload independently of Subject GET and is isolated by Workspace/Subject", () => {
  const values = new Map<string, string>();
  const storage = {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => {
      values.set(key, value);
    },
    removeItem: (key: string) => {
      values.delete(key);
    },
  };
  const attempt = {
    subjectId: "s",
    operationId: "same-op",
  };
  saveSubjectDeleteAttempt(storage, "w", attempt);
  equal(readSubjectDeleteAttempt(storage, "w", "s"), attempt);
  equal(readSubjectDeleteAttempt(storage, "other", "s"), null);
  equal(readSubjectDeleteAttempt(storage, "w", "other"), null);
  saveSubjectDeleteAttempt(storage, "other", attempt);
  clearSubjectDeleteAttempt(storage, "w", "s");
  equal(readSubjectDeleteAttempt(storage, "w", "s"), null);
  equal(readSubjectDeleteAttempt(storage, "other", "s"), attempt);
  values.set(subjectDeleteStorageKey("w", "s"), '{"subjectId":"other"}');
  try {
    readSubjectDeleteAttempt(storage, "w", "s");
  } catch {
    return;
  }
  throw new Error("Corrupt retry input allowed new deletion");
});
