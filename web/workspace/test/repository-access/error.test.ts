import fixture from "./fixtures/credential-not-found.json" with {
  type: "json",
};
import {
  RepositoryAccessHttpError,
  repositoryAccessResponseError,
} from "../../src/lib/workspace/api/repository-access-error.ts";

function equal(actual: unknown, expected: unknown) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `Expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}
const response = (body: unknown = fixture, status = 404) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/problem+json" },
  });

Deno.test("Repository management consumes the exact Rust HTTP fixture without losing reason or explanation", async () => {
  const error = await repositoryAccessResponseError(
    response(),
    "rotate credential",
  );
  equal(error instanceof RepositoryAccessHttpError, true);
  if (
    !(error instanceof RepositoryAccessHttpError) ||
    error.receipt.kind !== "problem"
  ) throw error;
  equal(error.message, fixture.detail);
  equal(error.receipt.problem.yoi_failure, fixture.yoi_failure);
  equal(error.receipt.problem.status, 404);
});

Deno.test("Repository management never derives execution, retry or outcome from changed explanation", async () => {
  for (
    const explanation of [
      "SUCCESS; automatically resend",
      "Unknown; do not resend",
    ]
  ) {
    const error = await repositoryAccessResponseError(
      response({
        ...fixture,
        detail: explanation,
        yoi_failure: { ...fixture.yoi_failure, explanation },
      }),
      "rotate credential",
    );
    if (
      !(error instanceof RepositoryAccessHttpError) ||
      error.receipt.kind !== "problem"
    ) throw error;
    equal(
      error.receipt.problem.yoi_failure.details,
      fixture.yoi_failure.details,
    );
    equal(error.receipt.problem.status, 404);
    equal(error.message, explanation);
  }
});

Deno.test("Unsupported and corrupt Repository errors stay distinct and do not expose raw payloads", async () => {
  for (
    const [patch, kind] of [
      [{ version: 65535 }, "unsupported"],
      [{
        details: {
          ...fixture.yoi_failure.details,
          provider_secret: "/private/provider/body",
        },
      }, "invalid"],
    ] as const
  ) {
    const error = await repositoryAccessResponseError(
      response({
        ...fixture,
        yoi_failure: { ...fixture.yoi_failure, ...patch },
      }),
      "rotate credential",
    );
    if (!(error instanceof RepositoryAccessHttpError)) throw error;
    equal(error.receipt.kind, kind);
    equal(error.message.includes("/private/provider/body"), false);
    equal(error.message.includes("Reload the page"), false);
  }
  const mismatch = await repositoryAccessResponseError(
    response(fixture, 409),
    "rotate credential",
  );
  if (!(mismatch instanceof RepositoryAccessHttpError)) throw mismatch;
  equal(mismatch.receipt.kind, "invalid");
});

Deno.test("Legacy authorization stays permission-specific, without projecting provider text", async () => {
  const error = await repositoryAccessResponseError(
    new Response("private provider body", { status: 403 }),
    "rotate credential",
  );
  equal(error.message, "You do not have permission to rotate credential.");
});
