import {
  loadRepositoryAccessJson,
  loadRepositoryAccessPermissionGate,
  loadRepositoryAccessSection,
} from "../../src/lib/workspace/api/repository-access-loader.ts";
import { RepositoryAccessSchemaError } from "../../src/lib/workspace/api/repository-access.ts";
import { load as loadRepositoryAccessPage } from "../../src/routes/w/[workspaceId]/settings/repository-access/+page.ts";

type HttpFailure = { status?: number; body?: { message?: string } };

type LoadedPage = {
  accessProjection: unknown;
  accessProjectionError: string | null;
  credentials: Array<{ credential_id: string }>;
  credentialsError: string | null;
  publicKeys: Array<{ credential_id: string }>;
  defaultPublicKeyError: string | null;
  hostTrusts: Array<{ host_trust_id: string }>;
  hostTrustsError: string | null;
};

const projectionFixture = {
  workspace_id: "workspace-1",
  config_revision: 1,
  projection_digest: "sha256:projection",
  bindings: [],
};
const credentialFixture = {
  credential_id: "workspace-default",
  workspace_id: "workspace-1",
  name: "Workspace default",
  public_key_algorithm: "ssh-ed25519",
  public_key_fingerprint: "SHA256:credential",
  current_revision: 1,
  status: "active",
  created_at: "2026-01-01T00:00:00Z",
  rotated_at: null,
  referenced_repositories: [],
};
const publicKeyFixture = {
  credential_id: "workspace-default",
  current_revision: 1,
  public_key_algorithm: "ssh-ed25519",
  public_key_fingerprint: "SHA256:credential",
  public_key: "ssh-ed25519 AAAA-workspace-default",
};
const hostTrustFixture = {
  host_trust_id: "github-com",
  workspace_id: "workspace-1",
  hostname: "github.com",
  port: 22,
  key_algorithm: "ssh-ed25519",
  host_key: "ssh-ed25519 AAAA-host",
  fingerprint: "SHA256:host",
  current_revision: 1,
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
  referenced_repositories: [],
};

async function loadPageWithFailure(failedSuffix: string): Promise<LoadedPage> {
  const fetcher = (input: RequestInfo | URL): Promise<Response> => {
    const path = String(input);
    if (path.endsWith(failedSuffix)) {
      return Promise.resolve(new Response(null, { status: 503 }));
    }
    if (path.endsWith("/settings/repository-access")) {
      return Promise.resolve(Response.json(projectionFixture));
    }
    if (path.endsWith("/credentials")) {
      return Promise.resolve(Response.json([credentialFixture]));
    }
    if (path.endsWith("/credentials/workspace-default/public-key")) {
      return Promise.resolve(Response.json(publicKeyFixture));
    }
    if (path.endsWith("/host-trusts")) {
      return Promise.resolve(Response.json([hostTrustFixture]));
    }
    throw new Error(`unexpected loader request: ${path}`);
  };
  return await (loadRepositoryAccessPage as unknown as (event: {
    fetch: typeof fetch;
    params: { workspaceId: string };
  }) => Promise<LoadedPage>)({
    fetch: fetcher as typeof fetch,
    params: { workspaceId: "workspace-1" },
  });
}

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

Deno.test("Repository Access page loader preserves successful siblings for every section failure", async () => {
  const projectionFailure = await loadPageWithFailure(
    "/settings/repository-access",
  );
  if (
    projectionFailure.accessProjection !== null ||
    !projectionFailure.accessProjectionError
  ) {
    throw new Error(
      "projection failure was not contained to Repository bindings",
    );
  }
  if (
    projectionFailure.credentials.length !== 1 ||
    projectionFailure.hostTrusts.length !== 1
  ) {
    throw new Error("projection failure hid a successful sibling section");
  }

  const credentialFailure = await loadPageWithFailure("/credentials");
  if (
    credentialFailure.credentials.length !== 0 ||
    !credentialFailure.credentialsError
  ) {
    throw new Error("credential failure was not contained to SSH credentials");
  }
  if (
    !credentialFailure.accessProjection ||
    credentialFailure.hostTrusts.length !== 1
  ) {
    throw new Error("credential failure hid a successful sibling section");
  }

  const hostFailure = await loadPageWithFailure("/host-trusts");
  if (hostFailure.hostTrusts.length !== 0 || !hostFailure.hostTrustsError) {
    throw new Error("host-trust failure was not contained to pinned host keys");
  }
  if (!hostFailure.accessProjection || hostFailure.credentials.length !== 1) {
    throw new Error("host-trust failure hid a successful sibling section");
  }

  const publicKeyFailure = await loadPageWithFailure(
    "/credentials/workspace-default/public-key",
  );
  if (
    publicKeyFailure.publicKeys.length !== 0 ||
    !publicKeyFailure.defaultPublicKeyError
  ) {
    throw new Error(
      "public-key failure was not contained to its credential row",
    );
  }
  if (
    publicKeyFailure.credentials.length !== 1 ||
    publicKeyFailure.hostTrusts.length !== 1
  ) {
    throw new Error("public-key failure hid successful sections");
  }
});

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

Deno.test("Repository Access permission gate contains projection failure so sibling sections can load", async () => {
  const result = await loadRepositoryAccessPermissionGate(
    () => Promise.resolve(new Response(null, { status: 503 })),
    "/api/w/workspace-1/settings/repository-access",
    (value) => value,
    "Repository bindings",
  );
  if (result.data !== null) {
    throw new Error("failed permission projection must not expose data");
  }
  if (result.error !== "Repository Access request failed with status 503.") {
    throw new Error(
      `unexpected contained projection error: ${String(result.error)}`,
    );
  }
});

Deno.test("Repository Access permission gate still aborts on forbidden access", async () => {
  await captureHttpFailure(
    () =>
      loadRepositoryAccessPermissionGate(
        () => Promise.resolve(new Response(null, { status: 403 })),
        "/api/w/workspace-1/settings/repository-access",
        (value) => value,
        "Repository bindings",
      ),
    403,
    "Repository Access is unavailable for this account.",
  );
});

Deno.test("Repository Access section loader contains one failed sibling", async () => {
  const result = await loadRepositoryAccessSection(
    () => Promise.resolve(new Response(null, { status: 503 })),
    "/api/w/workspace-1/settings/repository-access/credentials",
    (value) => value,
    "SSH credentials",
  );
  if (result.data !== null) {
    throw new Error("failed section must not expose data");
  }
  if (result.error !== "Repository Access request failed with status 503.") {
    throw new Error(`unexpected scoped error: ${String(result.error)}`);
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
