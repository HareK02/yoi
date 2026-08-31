import { loadRepositoryAccessJson } from "../../src/lib/workspace/api/repository-access-loader.ts";
import { RepositoryAccessSchemaError } from "../../src/lib/workspace/api/repository-access.ts";

type HttpFailure = { status?: number; body?: { message?: string } };

async function captureHttpFailure(
  run: () => Promise<unknown>,
  expectedStatus: number,
  expectedMessage: string,
): Promise<HttpFailure> {
  try {
    await run();
  } catch (error) {
    const failure = error as HttpFailure;
    if (failure.status !== expectedStatus) {
      throw new Error(
        `expected bounded ${expectedStatus}, got ${String(failure.status)}`,
      );
    }
    if (failure.body?.message !== expectedMessage) {
      throw new Error(
        `unexpected bounded error: ${JSON.stringify(failure.body)}`,
      );
    }
    return failure;
  }
  throw new Error(`expected bounded ${expectedStatus} error`);
}

for (const status of [401, 403]) {
  Deno.test(`Repository Access loader maps ${status} to bounded permission unavailable`, async () => {
    let requests = 0;
    await captureHttpFailure(
      () =>
        loadRepositoryAccessJson(
          () => {
            requests += 1;
            return Promise.resolve(new Response(null, { status }));
          },
          "/api/w/workspace-1/settings/repository-access",
          (value) => value,
        ),
      403,
      "Repository Access is unavailable for this account.",
    );
    if (requests !== 1) {
      throw new Error(`expected one bounded request, got ${requests}`);
    }
  });
}

Deno.test("Repository Access loader maps invalid JSON to safe bounded 502", async () => {
  const upstreamSecret = "private-key-must-not-leak";
  const failure = await captureHttpFailure(
    () =>
      loadRepositoryAccessJson(
        () =>
          Promise.resolve(
            new Response(upstreamSecret, {
              status: 200,
              headers: { "content-type": "application/json" },
            }),
          ),
        "/api/w/workspace-1/settings/repository-access/credentials",
        (value) => value,
      ),
    502,
    "Repository Access returned an invalid JSON response.",
  );
  if (JSON.stringify(failure.body).includes(upstreamSecret)) {
    throw new Error("invalid JSON error exposed upstream response content");
  }
});

Deno.test("Repository Access loader maps schema mismatch to explicit bounded 502", async () => {
  const failure = await captureHttpFailure(
    () =>
      loadRepositoryAccessJson(
        () => Promise.resolve(Response.json({ stale: true })),
        "/api/w/workspace-1/settings/repository-access/credentials",
        () => {
          throw new RepositoryAccessSchemaError("credentials", "an array");
        },
      ),
    502,
    "Repository Access response schema mismatch at credentials: expected an array",
  );
  if (!failure.body?.message?.includes("credentials")) {
    throw new Error("schema mismatch error omitted the failing response path");
  }
});

Deno.test("Repository Access loader never exposes failed upstream response bodies", async () => {
  const upstreamSecret = "secret-ref-must-not-leak";
  const failure = await captureHttpFailure(
    () =>
      loadRepositoryAccessJson(
        () =>
          Promise.resolve(
            Response.json(
              { message: upstreamSecret, secret_ref: upstreamSecret },
              { status: 500 },
            ),
          ),
        "/api/w/workspace-1/settings/repository-access/host-trusts",
        (value) => value,
      ),
    502,
    "Repository Access request failed with status 500.",
  );
  if (JSON.stringify(failure.body).includes(upstreamSecret)) {
    throw new Error("bounded upstream error exposed response content");
  }
});
