declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
  readTextFileSync(path: URL): string;
};

import {
  decodeFailure,
  decodeProblem,
  failureIdentifier,
  failureRecord,
  receiveProblem,
} from "./receive.ts";
import type { DetailsDecoder } from "./receive.ts";
import { failureNotice } from "./model.ts";
import { FAILURE_LIMITS } from "#lib/generated/error-contract.ts";

function equal(actual: unknown, expected: unknown) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `Expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}
// T810の二domain例はcoreの意味検証用。実Server認可/HTTP保証の代替ではない。
const fixtureDecoder: DetailsDecoder<{ operation: string }> = (envelope) => {
  if (envelope.details_version !== 1) return { kind: "unsupported" };
  const details = failureRecord(envelope.details);
  if (envelope.domain === "subject" && envelope.reason === "operation_denied") {
    if (envelope.category !== "authorization") return { kind: "invalid" };
    failureIdentifier(details.subject_ref);
    return {
      kind: "known",
      details: { operation: failureIdentifier(details.required_operation) },
    };
  }
  if (envelope.domain === "workdir" && envelope.reason === "in_use") {
    if (envelope.category !== "conflict") return { kind: "invalid" };
    failureIdentifier(details.workdir_ref);
    return {
      kind: "known",
      details: { operation: failureIdentifier(details.operation) },
    };
  }
  return { kind: "unsupported" };
};
const subject = JSON.parse(
  Deno.readTextFileSync(
    new URL(
      "../../../../../../crates/error-contract/fixtures/subject-denied.json",
      import.meta.url,
    ),
  ),
);
const workdir = JSON.parse(
  Deno.readTextFileSync(
    new URL(
      "../../../../../../crates/error-contract/fixtures/workdir-in-use.json",
      import.meta.url,
    ),
  ),
);
function problem(failure: unknown = subject) {
  return {
    type: "urn:yoi:problem:example",
    title: "Example",
    status: 409,
    detail: (failure as typeof subject).explanation,
    instance: "",
    yoi_failure: failure,
  };
}
function decode(value: unknown) {
  return decodeProblem(
    JSON.stringify(value),
    409,
    "application/problem+json; charset=utf-8",
    fixtureDecoder,
  );
}

Deno.test("T810 Rust/TS shared fixtures preserve cause, operation context and correlation", () => {
  for (const fixture of [subject, workdir]) {
    const received = decodeFailure(JSON.stringify(fixture), fixtureDecoder);
    if (received.kind !== "known") throw new Error(received.kind);
    equal(received.failure.domain, fixture.domain);
    equal(received.failure.reason, fixture.reason);
    equal(received.failure.context, fixture.context);
    equal(
      received.failure.details.operation,
      fixture.details.required_operation ?? fixture.details.operation,
    );
  }
});
Deno.test("explanatory wording, language, RFC type and unknown extensions do not select operation results", () => {
  for (
    const explanation of [
      "拒否されたため登録できません。",
      "OutcomeUnknown is only text",
      "A new explanation",
    ]
  ) {
    const value = problem({ ...subject, explanation });
    value.title = "Unbekannter Fehler";
    value.type = "urn:future:problem";
    const received = decode({
      ...value,
      future_extension: { source: "/internal/secret", token: "secret" },
    });
    if (received.kind !== "problem") throw new Error(received.kind);
    equal(received.problem.yoi_failure.details.operation, "session.record");
    const displayed = failureNotice(received);
    equal(displayed.explanation, explanation);
    equal(JSON.stringify(displayed).includes("secret"), false);
    equal("retryable" in displayed, false);
  }
});
Deno.test("unsupported required contracts never become known outcomes or display opaque payloads", () => {
  for (
    const failure of [
      { ...subject, version: 2 },
      { ...subject, required_extensions: ["future"] },
      { ...subject, details_version: 2 },
      { ...subject, reason: "future" },
    ]
  ) {
    const received = decode(problem(failure));
    equal(received.kind, "unsupported");
    equal(
      JSON.stringify(failureNotice(received)).includes("session.record"),
      false,
    );
  }
});
Deno.test("invalid identity, status mismatch, missing/malformed fields, duplicate fields and depth reject safely", () => {
  for (
    const value of [
      { ...problem(), status: 403 },
      { ...problem(), detail: "other" },
      { ...problem(), yoi_failure: null },
      { ...problem(), title: null },
      problem({ ...subject, category: "internal" }),
      problem({
        ...subject,
        context: {
          ...subject.context,
          operations: Array(9).fill(subject.context.operations[0]),
        },
      }),
      problem({ ...subject, reason: "bad\nreason" }),
      problem({ ...subject, explanation: "あ".repeat(700) }),
    ]
  ) equal(decode(value).kind, "invalid");
  equal(
    decodeProblem(
      '{"status":403,' + JSON.stringify(problem()).slice(1),
      409,
      "application/problem+json",
      fixtureDecoder,
    ).kind,
    "invalid",
  );
  equal(
    decode({
      ...problem(),
      unknown: JSON.parse("[".repeat(17) + "0" + "]".repeat(17)),
    }).kind,
    "invalid",
  );
  equal(
    decodeProblem(
      "raw /private/path token=secret",
      500,
      "application/problem+json",
      fixtureDecoder,
    ).kind,
    "invalid",
  );
});
Deno.test("display uses server cause once, supplementary diagnostics separately, never raw details or source chain", () => {
  const received = decode(
    problem({
      ...subject,
      details: { ...subject.details, source: "secret" },
      context: {
        ...subject.context,
        diagnostics: [
          { code: "wrapper", explanation: subject.explanation },
          {
            code: "additional",
            explanation: "Saved results remain available.",
          },
        ],
      },
    }),
  );
  const display = failureNotice(received);
  equal(display.supplements, ["Saved results remain available."]);
  equal(display.explanation, subject.explanation);
  equal(JSON.stringify(display).includes("secret"), false);
});
Deno.test("stream byte budgets cancel before JSON parse with and without content-length", async () => {
  for (
    const headers of [
      { "content-length": String(FAILURE_LIMITS.bytes + 1) },
      {},
    ] as Record<string, string>[]
  ) {
    let cancelled = false;
    const stream = new ReadableStream<Uint8Array>({
      pull(controller) {
        controller.enqueue(new Uint8Array(1024));
      },
      cancel() {
        cancelled = true;
      },
    });
    const response = new Response(stream, {
      status: 500,
      headers: { ...headers, "content-type": "application/problem+json" },
    });
    equal((await receiveProblem(response, fixtureDecoder)).kind, "too_large");
    equal(cancelled, true);
  }
});
Deno.test("bounded response text, unknown MIME, broken transport and UTF8 never expose raw bodies", async () => {
  equal(
    (await receiveProblem(
      new Response(JSON.stringify(problem()), {
        status: 409,
        headers: { "content-type": "application/problem+json" },
      }),
      fixtureDecoder,
    )).kind,
    "problem",
  );
  const cases = [
    new Response("<html>secret</html>", { status: 502 }),
    new Response(new Uint8Array([255]), {
      status: 502,
      headers: { "content-type": "application/problem+json" },
    }),
    new Response(
      new ReadableStream({
        start(controller) {
          controller.error(new Error("secret transport path"));
        },
      }),
      { status: 502, headers: { "content-type": "application/problem+json" } },
    ),
  ];
  for (const [index, response] of cases.entries()) {
    const received = await receiveProblem(response, fixtureDecoder);
    equal(received.kind, ["http", "invalid", "transport"][index]);
    if (received.kind === "problem") throw new Error("unexpected problem");
    equal(JSON.stringify(failureNotice(received)).includes("secret"), false);
  }
});

Deno.test("Problem URI references are syntax checked without normalization, navigation or fetching", () => {
  for (
    const reference of [
      "",
      "../requests/1",
      "?occurrence=1",
      "#s",
      "g:h",
      "a/b:c",
      "///path",
      "https://user:pass@example.test:443/a?b#c",
      "//[2001:db8::1]:443/a",
      "//[::ffff:192.0.2.128]/",
      "//[v1.a:b]/x",
      "a%20b/%5B?x=%25&y=?/#%2f",
    ]
  ) {
    const received = decode({
      ...problem(),
      type: reference,
      instance: reference,
    });
    if (received.kind !== "problem") {
      throw new Error(`${reference}: ${received.kind}`);
    }
    equal(received.problem.instance, reference);
  }
  for (
    const reference of [
      "/requests/%GG",
      "/requests/[invalid",
      "foo%",
      "//user@@host/x",
      "//host:wrongport/x",
      "//[::not-ipv6]/",
      "//[v.a]/",
      "//[vG.foo]/",
      "//[v1.]/",
      "//[::1]tail/",
      "//[::1",
      "//host]/",
      "bad space",
      "1invalid:rest",
      ":rest",
      "a{b",
      "a#b#c",
      "a\\b",
      "日本語",
      "/a\n",
      "?x=\0",
    ]
  ) {
    equal(
      decode({ ...problem(), type: reference, instance: reference }).kind,
      "invalid",
    );
  }
});

Deno.test("all shared collection budgets and UTF8 identifiers are validated before presentation", () => {
  for (
    const [key, max, item] of [
      ["operations", 8, subject.context.operations[0]],
      ["causes", 4, {
        domain: "subject",
        reason: "operation_denied",
        correlation: "safe",
      }],
      ["diagnostics", 8, {
        code: "safe",
        explanation: "Additional information",
      }],
    ] as const
  ) {
    equal(
      decode(
        problem({
          ...subject,
          context: { ...subject.context, [key]: Array(max).fill(item) },
        }),
      ).kind,
      "problem",
    );
    equal(
      decode(
        problem({
          ...subject,
          context: { ...subject.context, [key]: Array(max + 1).fill(item) },
        }),
      ).kind,
      "invalid",
    );
  }
  const received = decode(
    problem({
      ...subject,
      context: { ...subject.context, supplementary_omitted: true },
    }),
  );
  equal(failureNotice(received).supplements, [
    "Some additional information was omitted by the server.",
  ]);
  equal(
    decode(
      problem({
        ...subject,
        context: { ...subject.context, correlation: "あ".repeat(43) },
      }),
    ).kind,
    "invalid",
  );
  equal(
    decodeProblem(
      " ".repeat(FAILURE_LIMITS.bytes) + JSON.stringify(problem()),
      409,
      "application/problem+json",
      fixtureDecoder,
    ).kind,
    "too_large",
  );
});

Deno.test("JSON Unicode scalars agree with the Rust wire reader rather than replacing lone surrogates", () => {
  for (const explanation of ["bad\ud800", "bad\udfff"]) {
    equal(decode(problem({ ...subject, explanation })).kind, "invalid");
  }
  equal(
    decode(
      problem({ ...subject, explanation: "Valid supplementary scalar: 🚀" }),
    ).kind,
    "problem",
  );
});
