// @vitest-environment happy-dom
// BrowserAuth→本番adapterの競合/結果判定。HTTPと認証器のみをgate付きで代替し、
// Server検証/保存の根拠にはしない。秘密の本文は診断へ保存しない。
import { afterEach, expect, test, vi } from "vitest";
import { BrowserAuth } from "./browser-session.svelte.ts";
import { browserSession, sessionMutation } from "./session-state.ts";
import { addPasskey, loginWithPasskey, logout } from "./api.ts";
import {
  authUser,
  credentialFixture,
  loginOptions,
  registrationOptions,
} from "./api.test-fixtures.ts";

const owners: BrowserAuth[] = [];
afterEach(() => {
  for (const owner of owners.splice(0)) owner.dispose();
  browserSession.invalidate();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});
function gate<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}
function identity(user: typeof authUser | null) {
  return Response.json({
    authentication_reference: user ? "fixture-authentication" : null,
    actor: user ? { ...user, auth_method: "browser_session" } : null,
    session: user
      ? { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" }
      : { kind: "anonymous" },
  });
}
function harness() {
  const state = { user: authUser as typeof authUser | null, completed: false };
  const create = vi.fn<CredentialsContainer["create"]>().mockResolvedValue(
    credentialFixture(),
  );
  const get = vi.fn<CredentialsContainer["get"]>().mockResolvedValue(
    credentialFixture(true),
  );
  vi.stubGlobal("navigator", { credentials: { create, get } });
  const fetcher = vi.fn<typeof fetch>().mockImplementation(async (input) => {
    const path = String(input);
    if (path === "/api/auth/whoami") return identity(state.user);
    if (path.endsWith("/options")) {
      return Response.json(
        path.includes("login") ? loginOptions : registrationOptions,
      );
    }
    if (path.endsWith("/complete")) {
      state.completed = true;
      return Response.json({
        user: authUser,
        authentication_reference: "fixture-authentication",
      });
    }
    if (path === "/api/auth/logout") {
      state.user = null;
      return Response.json({ status: "logged_out" });
    }
    throw new Error("Unexpected synthetic endpoint");
  });
  vi.stubGlobal("fetch", fetcher);
  const auth = new BrowserAuth();
  owners.push(auth);
  return { state, create, get, fetcher, auth };
}
for (const outcome of ["anonymous", "other-actor", "unavailable"] as const) {
  test(`addition response is not Added when confirming whoami is ${outcome}`, async () => {
    const { auth, state, fetcher } = harness();
    await auth.refresh();
    const request = fetcher.getMockImplementation()!;
    fetcher.mockImplementation(async (input, init) => {
      if (state.completed && String(input) === "/api/auth/whoami") {
        if (outcome === "unavailable") {
          throw new TypeError("private network detail");
        }
        return identity(
          outcome === "anonymous" ? null : {
            ...authUser,
            user_id: "bob-user",
            account_id: "bob-account",
            handle: "bob",
          },
        );
      }
      return await request(input, init);
    });
    expect(await auth.run("Waiting", "Added", addPasskey)).toBe(false);
    expect(auth.status).not.toBe("Added");
    expect(auth.error).not.toContain("private network detail");
  });
}
for (const invalidation of ["logout", "dispose", "account-change"] as const) {
  test(`${invalidation} during native prompt never posts a delayed login credential`, async () => {
    const { auth, state, get, fetcher } = harness();
    state.user = null;
    await auth.refresh();
    const prompted = gate<void>();
    const credential = gate<Credential | null>();
    get.mockImplementation(() => {
      prompted.resolve();
      return credential.promise;
    });
    const run = auth.run("Waiting", "Logged in", loginWithPasskey);
    await prompted.promise;
    if (invalidation === "dispose") auth.dispose();
    else if (invalidation === "account-change") sessionMutation();
    else {
      const signout = new BrowserAuth();
      owners.push(signout);
      await signout.run("Logging out", "Logged out", logout, "logout");
    }
    credential.resolve(credentialFixture(true));
    expect(await run).toBe(false);
    expect(
      fetcher.mock.calls.some(([path]) =>
        String(path).endsWith("/login/complete")
      ),
    ).toBe(false);
    expect(state.completed).toBe(false);
  });
}

const bob = {
  ...authUser,
  user_id: "bob-user",
  account_id: "bob-account",
  handle: "bob",
};
for (const changeAt of ["start", "before-complete"] as const) {
  test(`stale Alice with Bob cookie at ${changeAt} cannot add to Bob`, async () => {
    const { auth, state, create, fetcher } = harness();
    await auth.refresh();
    if (changeAt === "start") state.user = bob;
    else {create.mockImplementationOnce(async () => {
        state.user = bob;
        return credentialFixture();
      });}
    expect(await auth.run("Waiting", "Added", addPasskey)).toBe(false);
    expect(state.completed).toBe(false);
    expect(
      fetcher.mock.calls.some(([path]) => String(path).endsWith("/complete")),
    ).toBe(false);
    if (changeAt === "start") expect(create).not.toHaveBeenCalled();
    expect(auth.error).toContain("session-changed");
  });
}
test("same owner but different authentication binding before complete is not the expected session", async () => {
  const { auth, create, fetcher } = harness();
  await auth.refresh();
  let changed = false;
  create.mockImplementationOnce(async () => {
    changed = true;
    return credentialFixture();
  });
  const request = fetcher.getMockImplementation()!;
  fetcher.mockImplementation(async (input, init) =>
    String(input) === "/api/auth/whoami" && changed
      ? Response.json({
        ...(await identity(authUser).json()),
        authentication_reference: "different-binding",
      })
      : request(input, init)
  );
  expect(await auth.run("Waiting", "Added", addPasskey)).toBe(false);
  expect(
    fetcher.mock.calls.some(([path]) => String(path).endsWith("/complete")),
  ).toBe(false);
});
test("returned owner alone does not prove the issued browser session", async () => {
  const { auth, fetcher } = harness();
  await auth.refresh();
  const request = fetcher.getMockImplementation()!;
  fetcher.mockImplementation(async (input, init) =>
    String(input).endsWith("/complete")
      ? Response.json({
        user: authUser,
        authentication_reference: "issued-but-not-current",
      })
      : request(input, init)
  );
  expect(await auth.run("Waiting", "Added", addPasskey)).toBe(false);
  expect(auth.error).toContain("expected browser session is not confirmed");
});
for (
  const reason of [
    "InvalidStateError",
    "NotAllowedError",
    "SecurityError",
    "NotSupportedError",
  ] as const
) {
  test(`native ${reason} has a safe stage-specific classification without raw exceptions`, async () => {
    const { auth, create } = harness();
    await auth.refresh();
    create.mockRejectedValueOnce(
      new DOMException("RAW-CREDENTIAL-TOKEN-COOKIE", reason),
    );
    expect(await auth.run("Waiting", "Added", addPasskey)).toBe(false);
    expect(auth.error).not.toContain("RAW-CREDENTIAL-TOKEN-COOKIE");
    expect(auth.error).toContain("authenticator/");
    expect(auth.error).toContain("Completion was not sent");
    if (reason === "InvalidStateError") {
      expect(auth.error).toContain("duplicate exclusion");
    }
    if (reason === "NotAllowedError") {
      expect(auth.error).toContain("does not distinguish");
    }
  });
}
test("device creation followed by conversion failure never sends complete or claims registration", async () => {
  const { auth, create, fetcher } = harness();
  await auth.refresh();
  create.mockResolvedValueOnce(
    {
      ...credentialFixture(),
      rawId: "private-invalid-value",
    } as unknown as Credential,
  );
  expect(await auth.run("Waiting", "Added", addPasskey)).toBe(false);
  expect(auth.error).toContain("conversion");
  expect(auth.error).toContain("device may have created");
  expect(auth.error).not.toContain("private-invalid-value");
  expect(
    fetcher.mock.calls.some(([path]) => String(path).endsWith("/complete")),
  ).toBe(false);
});
test("duplicate operations are fenced and explicit cancellation stops late credentials before submission", async () => {
  const { auth, create, fetcher } = harness();
  await auth.refresh();
  const started = gate<void>(), credential = gate<Credential | null>();
  create.mockImplementationOnce(() => {
    started.resolve();
    return credential.promise;
  });
  const first = auth.run("Waiting", "Added", addPasskey);
  await started.promise;
  const duplicate = vi.fn(async () => ({}));
  expect(await auth.run("Duplicate", "Wrong", duplicate)).toBe(false);
  expect(duplicate).not.toHaveBeenCalled();
  auth.cancel();
  credential.resolve(credentialFixture());
  expect(await first).toBe(false);
  expect(
    fetcher.mock.calls.some(([path]) => String(path).endsWith("/complete")),
  ).toBe(false);
});
test("abort after complete dispatch remains an unknown submitted addition with a recoverable operation", async () => {
  const { auth, fetcher } = harness();
  await auth.refresh();
  const started = gate<void>(), response = gate<Response>();
  const request = fetcher.getMockImplementation()!;
  fetcher.mockImplementation((input, init) => {
    if (String(input).endsWith("/complete")) {
      started.resolve();
      return response.promise;
    }
    return request(input, init);
  });
  const run = auth.run("Waiting", "Added", addPasskey);
  await started.promise;
  auth.cancel();
  response.resolve(
    Response.json({
      user: authUser,
      authentication_reference: "fixture-authentication",
    }),
  );
  expect(await run).toBe(false);
  expect(auth.additionAttempt).not.toBeNull();
  expect(auth.error).toContain("result is unknown");
  expect(auth.error).not.toContain("Completion was not sent");
});
for (
  const current of [
    "original-session",
    "cookie-updated",
    "anonymous",
    "bob",
    "unavailable",
  ] as const
) {
  test(`lost completion body with ${current} distinguishes saved, session evidence and unavailable authority`, async () => {
    const { auth, state, fetcher } = harness();
    await auth.refresh();
    let dispatched = false, resultReads = 0;
    const request = fetcher.getMockImplementation()!;
    fetcher.mockImplementation(async (input, init) => {
      const path = String(input);
      if (path.endsWith("/complete")) {
        dispatched = true;
        throw new TypeError("RAW-HTTP-BODY-TOKEN");
      }
      if (path === "/api/auth/whoami" && dispatched) {
        if (current === "unavailable") throw new TypeError("RAW-WHOAMI-ERROR");
        if (current === "anonymous") return identity(null);
        if (current === "bob") return identity(bob);
        if (current === "cookie-updated") {
          return Response.json({
            ...(await identity(authUser).json()),
            authentication_reference: "new-live-session",
          });
        }
      }
      if (path.endsWith("/addition/result")) {
        ++resultReads;
        expect(JSON.parse(String(init?.body)).expected_authentication_reference)
          .toBe("fixture-authentication");
        return Response.json({
          status: "saved",
          user: authUser,
          key: {
            key_reference: "safe-opaque-reference",
            created_at: "2026-01-01T00:00:00Z",
            last_used_at: null,
            normal_login_verified_at: null,
          },
        });
      }
      return request(input, init);
    });
    expect(await auth.run("Waiting", "Added", addPasskey)).toBe(false);
    expect(auth.additionAttempt).not.toBeNull();
    expect(auth.error).not.toContain("RAW-");
    expect(auth.error).toContain("result is unknown");
    await auth.checkAddition();
    if (current === "original-session" || current === "cookie-updated") {
      expect(auth.additionSaved).toBe(true);
      expect(auth.status).toContain("saved a key");
      expect(auth.status).toContain(
        "session issued by completion is not confirmed",
      );
      expect(resultReads).toBe(1);
    } else {
      expect(auth.additionSaved).toBe(false);
      expect(resultReads).toBe(0);
      expect(auth.error).toContain("does not prove failure");
    }
    expect(state.completed).toBe(false); // synthetic boundary never acts as the real store
  });
}

test("storage mutation arriving before the storage event fences prompt completion even if whoami stays anonymous", async () => {
  const { auth, state, get, fetcher } = harness();
  state.user = null;
  await auth.refresh();
  get.mockImplementationOnce(async () => {
    localStorage.setItem(
      "yoi.browser-auth-generation",
      JSON.stringify({ id: "another-tab", reason: "logout" }),
    );
    return credentialFixture(true);
  });
  expect(await auth.run("Waiting", "Logged in", loginWithPasskey)).toBe(false);
  expect(
    fetcher.mock.calls.some(([path]) =>
      String(path).endsWith("/login/complete")
    ),
  ).toBe(false);
});

for (const phase of ["options", "complete"] as const) {
  test(`malformed ${phase} response records the stage without leaking body or inventing completion`, async () => {
    const { auth, fetcher } = harness();
    await auth.refresh();
    const request = fetcher.getMockImplementation()!;
    fetcher.mockImplementation((input, init) =>
      String(input).endsWith(`/${phase}`)
        ? Promise.resolve(
          new Response("RAW-CHALLENGE-COOKIE", {
            headers: { "content-type": "application/json" },
          }),
        )
        : request(input, init)
    );
    expect(await auth.run("Waiting", "Added", addPasskey)).toBe(false);
    expect(auth.error).toContain(`${phase}/invalid-response`);
    expect(auth.error).not.toContain("RAW-CHALLENGE-COOKIE");
    expect(auth.additionAttempt === null).toBe(phase === "options");
    if (phase === "options") {
      expect(auth.error).toContain("Completion was not sent");
    } else expect(auth.error).toContain("result is unknown");
  });
}
for (
  const code of [
    "passkey_authentication_changed",
    "webauthn_registration_verification_failed",
  ] as const
) {
  test(`HTTP rejection classifies allowlisted ${code} without retaining HTTP body`, async () => {
    const { auth, fetcher } = harness();
    await auth.refresh();
    const request = fetcher.getMockImplementation()!;
    fetcher.mockImplementation((input, init) =>
      String(input).endsWith("/complete")
        ? Promise.resolve(
          Response.json({
            message: `${code}: RAW-EXCEPTION-BODY`,
            secret: "RAW-TOKEN",
          }, { status: 401 }),
        )
        : request(input, init)
    );
    expect(await auth.run("Waiting", "Added", addPasskey)).toBe(false);
    expect(auth.error).not.toContain("RAW-");
    expect(auth.error).toContain(
      code === "passkey_authentication_changed"
        ? "session-changed"
        : "rejected",
    );
    expect(auth.error).toContain("HTTP-401");
    expect(auth.error).not.toContain("Completion was not sent");
  });
}

test("successful logout never marks an earlier unknown addition as saved", async () => {
  const { auth, fetcher } = harness();
  await auth.refresh();
  const request = fetcher.getMockImplementation()!;
  fetcher.mockImplementation((input, init) =>
    String(input).endsWith("/complete")
      ? Promise.reject(new TypeError("response lost"))
      : request(input, init)
  );
  expect(await auth.run("Waiting", "Added", addPasskey)).toBe(false);
  expect(auth.additionAttempt).not.toBeNull();
  expect(auth.additionSaved).toBe(false);
  expect(await auth.run("Logging out", "Logged out", logout, "logout")).toBe(
    true,
  );
  expect(auth.additionSaved).toBe(false);
});

test("explicit login from stale expired display remains allowed after another tab cleared the cookie", async () => {
  const { auth, state, fetcher } = harness();
  state.user = null;
  fetcher.mockResolvedValueOnce(
    Response.json({
      actor: null,
      authentication_reference: null,
      session: { kind: "expired" },
    }),
  );
  await auth.refresh();
  const request = fetcher.getMockImplementation()!;
  fetcher.mockImplementation(async (input, init) => {
    if (String(input).endsWith("/complete")) state.user = authUser;
    return await request(input, init);
  });
  expect(await auth.run("Waiting", "Logged in", loginWithPasskey)).toBe(true);
  expect(auth.status).toBe("Logged in");
});
test("explicit login from expired display does not replace another tab's authenticated account", async () => {
  const { auth, state, fetcher, get } = harness();
  fetcher.mockResolvedValueOnce(
    Response.json({
      actor: null,
      authentication_reference: null,
      session: { kind: "expired" },
    }),
  );
  await auth.refresh();
  state.user = bob;
  expect(await auth.run("Waiting", "Logged in", loginWithPasskey)).toBe(false);
  expect(get).not.toHaveBeenCalled();
  expect(state.completed).toBe(false);
});
