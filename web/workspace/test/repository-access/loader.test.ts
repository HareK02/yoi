import { loadRepositoryAccessJson } from "../../src/lib/workspace/api/repository-access-loader.ts";

Deno.test("Repository Access loader maps missing permission to a bounded unavailable error", async () => {
  let requests = 0;
  try {
    await loadRepositoryAccessJson(
      () => {
        requests += 1;
        return Promise.resolve(new Response(null, { status: 403 }));
      },
      "/api/w/workspace-1/settings/repository-access/credentials",
      (value) => value,
    );
  } catch (error) {
    const failure = error as { status?: number; body?: { message?: string } };
    if (failure.status !== 403) {
      throw new Error(`expected bounded 403, got ${String(failure.status)}`);
    }
    if (
      failure.body?.message !==
        "Repository Access is unavailable for this account."
    ) {
      throw new Error(
        `unexpected permission error: ${JSON.stringify(failure.body)}`,
      );
    }
    if (requests !== 1) {
      throw new Error(`expected one bounded request, got ${requests}`);
    }
    return;
  }
  throw new Error("expected Repository Access permission error");
});
