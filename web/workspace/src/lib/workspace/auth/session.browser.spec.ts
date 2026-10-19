// @vitest-environment happy-dom
import { afterEach, expect, test, vi } from "vitest";
import { browserSession, sessionMutation } from "./session-state.ts";
import { BrowserAuth } from "./browser-session.svelte.ts";
import { addPasskey, loginWithPasskey, logout } from "./api.ts";
import { SESSION_CHANGED_EVENT } from "./hosting.ts";
import {
  authUser,
  credentialFixture,
  loginOptions,
  registrationOptions,
} from "./api.test-fixtures.ts";

const sessions: BrowserAuth[] = [];
afterEach(() => {
  for (const auth of sessions.splice(0)) auth.dispose();
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

function session() {
  const auth = new BrowserAuth();
  sessions.push(auth);
  return auth;
}

function whoami(authenticated: boolean) {
  return Response.json({
    session: authenticated
      ? { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" }
      : { kind: "anonymous" },
    actor: authenticated
      ? { ...authUser, auth_method: "browser_session" }
      : null,
  });
}

// BrowserAuth→本番HTTP/WebAuthn adapterの状態遷移を検証する。fetch/credentialだけを代替し、
// 遅延応答は明示的なgateで順序を決める。実Backendの認証・永続化を保証する試験ではない。
function boundary(authenticated = false) {
  const state = { authenticated, completeStatus: 200 };
  const create = vi.fn<CredentialsContainer["create"]>()
    .mockResolvedValue(credentialFixture());
  const get = vi.fn<CredentialsContainer["get"]>()
    .mockResolvedValue(credentialFixture(true));
  vi.stubGlobal("navigator", { credentials: { create, get } });
  const fetcher = vi.fn<typeof fetch>().mockImplementation(async (input) => {
    const path = String(input);
    if (path === "/api/auth/whoami") return whoami(state.authenticated);
    if (path === "/api/auth/logout") {
      state.authenticated = false;
      return Response.json({ status: "logged_out" });
    }
    if (path === "/api/auth/passkeys/login/options") {
      return Response.json(loginOptions);
    }
    if (path === "/api/auth/passkeys/addition/options") {
      return Response.json(registrationOptions);
    }
    if (
      path === "/api/auth/passkeys/login/complete" ||
      path === "/api/auth/passkeys/addition/complete"
    ) {
      if (state.completeStatus !== 200) {
        if (state.completeStatus === 401) state.authenticated = false;
        return new Response(null, { status: state.completeStatus });
      }
      state.authenticated = true;
      return Response.json({ user: authUser });
    }
    throw new Error(`Unexpected API: ${path}`);
  });
  vi.stubGlobal("fetch", fetcher);
  return { fetcher, create, get, state };
}

function paths(fetcher: ReturnType<typeof boundary>["fetcher"]) {
  return fetcher.mock.calls.map(([path]) => String(path));
}

test("loading and failed whoami are not anonymous readiness and cannot start authentication", async () => {
  const { fetcher } = boundary();
  const response = gate<Response>();
  fetcher.mockReturnValueOnce(response.promise);
  const auth = session();
  const operation = vi.fn(async () => {});
  expect(auth.session.kind).toBe("loading");
  const refresh = auth.refresh();
  expect(auth.actor).toBeNull();
  expect(await auth.run("pending", "done", operation)).toBe(false);
  response.resolve(new Response(null, { status: 503 }));
  await refresh;
  expect(auth.session).toEqual({ kind: "error" });
  expect(await auth.run("pending", "done", operation)).toBe(false);
  expect(operation).not.toHaveBeenCalled();
  await auth.refresh();
  expect(auth.session).toEqual({
    kind: "ready",
    actor: null,
    session: { kind: "anonymous" },
  });
});

for (
  const [olderStatus, latestStatus, latestAuthenticated] of [
    [200, 200, false],
    [200, 503, false],
    [503, 200, true],
  ] as const
) {
  test(`latest whoami (${latestStatus}, actor=${latestAuthenticated}) wins over a delayed ${olderStatus} response`, async () => {
    const { fetcher } = boundary();
    const older = gate<Response>();
    const latest = gate<Response>();
    fetcher.mockReturnValueOnce(older.promise).mockReturnValueOnce(
      latest.promise,
    );
    const auth = session();
    const first = auth.refresh();
    browserSession.invalidate();
    const second = auth.refresh();
    latest.resolve(
      latestStatus === 200
        ? whoami(latestAuthenticated)
        : new Response(null, { status: latestStatus }),
    );
    await second;
    expect(auth.session.kind).toBe(latestStatus === 200 ? "ready" : "error");
    expect(auth.actor?.handle ?? null).toBe(
      latestAuthenticated ? "alice" : null,
    );
    const accepted = auth.session;
    older.resolve(
      olderStatus === 200
        ? whoami(true)
        : new Response(null, { status: olderStatus }),
    );
    await first;
    expect(auth.session).toBe(accepted);
  });
}

test("a delayed pre-logout whoami cannot resurrect the previous actor", async () => {
  const { fetcher } = boundary(true);
  const auth = session();
  await auth.refresh();
  const oldResponse = gate<Response>();
  fetcher.mockReturnValueOnce(oldResponse.promise);
  const oldRefresh = auth.refresh();
  expect(await auth.run("Logging out", "Logged out", logout, "logout")).toBe(
    true,
  );
  expect(auth.session).toEqual({
    kind: "ready",
    actor: null,
    session: { kind: "anonymous" },
  });
  oldResponse.resolve(whoami(true));
  await oldRefresh;
  expect(auth.session).toEqual({
    kind: "ready",
    actor: null,
    session: { kind: "anonymous" },
  });
  expect(auth.status).toBe("Logged out");
});

test("busy login rejects duplicate operations and refresh until its post-completion whoami settles", async () => {
  const { fetcher } = boundary();
  const auth = session();
  await auth.refresh();
  const options = gate<Response>();
  const confirming = gate<Response>();
  const confirmingStarted = gate<void>();
  const request = fetcher.getMockImplementation()!;
  fetcher.mockImplementation((input, init) => {
    if (String(input) === "/api/auth/whoami") {
      confirmingStarted.resolve();
      return confirming.promise;
    }
    return request(input, init);
  });
  fetcher.mockReturnValueOnce(options.promise);
  const run = auth.run(
    "Waiting",
    "Logged in",
    (signal) => loginWithPasskey(signal),
  );
  expect(auth.busy).toBe(true);
  expect(auth.status).toBe("Waiting");
  const duplicate = vi.fn(async () => {});
  expect(await auth.run("duplicate", "incorrect", duplicate)).toBe(false);
  await auth.refresh();
  expect(duplicate).not.toHaveBeenCalled();
  expect(paths(fetcher)).toEqual([
    "/api/auth/whoami",
    "/api/auth/passkeys/login/options",
  ]);
  options.resolve(Response.json(loginOptions));
  await confirmingStarted.promise;
  expect(auth.busy).toBe(true);
  expect(auth.session.kind).toBe("ready");
  expect(await auth.run("duplicate", "incorrect", duplicate)).toBe(false);
  await auth.refresh();
  expect(paths(fetcher).filter((path) => path === "/api/auth/whoami"))
    .toHaveLength(2);
  confirming.resolve(whoami(true));
  expect(await run).toBe(true);
  expect(auth.busy).toBe(false);
  expect(auth.actor?.handle).toBe("alice");
  expect(auth.status).toBe("Logged in");
  expect(paths(fetcher)).toEqual([
    "/api/auth/whoami",
    "/api/auth/passkeys/login/options",
    "/api/auth/passkeys/login/complete",
    "/api/auth/whoami",
  ]);
});

for (const status of [400, 401, 410]) {
  test(`addition completion ${status} returns failure, clears success and rechecks the session`, async () => {
    const { fetcher, state } = boundary(true);
    const auth = session();
    await auth.refresh();
    state.completeStatus = status;
    const result = await auth.run("Waiting", "Added", addPasskey);
    expect(result).toBe(false);
    expect(auth.busy).toBe(false);
    expect(auth.status).toBe("");
    expect(auth.error).toMatch(
      status === 401
        ? /Authentication request was rejected/
        : /The request or authentication challenge is invalid or expired/,
    );
    expect(auth.session.kind).toBe("ready");
    expect(auth.actor?.handle ?? null).toBe(status === 401 ? null : "alice");
    expect(paths(fetcher)).toEqual([
      "/api/auth/whoami",
      "/api/auth/passkeys/addition/options",
      "/api/auth/passkeys/addition/complete",
      "/api/auth/whoami",
    ]);
  });
}

for (
  const name of [
    "NotAllowedError",
    "NotSupportedError",
    "AbortError",
    "Error",
  ] as const
) {
  test(`credential ${name} is sanitized, never completed, and leaves explicit retry possible`, async () => {
    const { fetcher, get } = boundary();
    const auth = session();
    await auth.refresh();
    get.mockRejectedValueOnce(
      name === "Error"
        ? new Error("secret credential detail")
        : new DOMException("secret credential detail", name),
    );
    expect(
      await auth.run(
        "Waiting",
        "Logged in",
        (signal) => loginWithPasskey(signal),
      ),
    ).toBe(false);
    expect(auth.error).toMatch(
      name === "NotAllowedError"
        ? /cancelled or timed out/
        : name === "NotSupportedError"
        ? /Completion is not confirmed/
        : name === "AbortError"
        ? /Completion is not confirmed/
        : /Authentication could not be confirmed/,
    );
    expect(auth.error).not.toContain("secret credential detail");
    expect(auth.status).toBe("");
    expect(auth.busy).toBe(false);
    expect(auth.session).toEqual({
      kind: "ready",
      actor: null,
      session: { kind: "anonymous" },
    });
    expect(paths(fetcher)).toEqual([
      "/api/auth/whoami",
      "/api/auth/passkeys/login/options",
      "/api/auth/whoami",
    ]);
    expect(
      await auth.run(
        "Waiting",
        "Logged in",
        (signal) => loginWithPasskey(signal),
      ),
    ).toBe(true);
    expect(auth.error).toBe("");
    expect(auth.actor?.handle).toBe("alice");
  });
}

test("completion does not confirm authentication when the subsequent whoami fails", async () => {
  const { fetcher } = boundary();
  const auth = session();
  await auth.refresh();
  fetcher.mockImplementation(async (input) => {
    const path = String(input);
    if (path === "/api/auth/whoami") return new Response(null, { status: 503 });
    if (path === "/api/auth/passkeys/login/options") {
      return Response.json(loginOptions);
    }
    if (path === "/api/auth/passkeys/login/complete") {
      return Response.json({ user: authUser });
    }
    throw new Error(`Unexpected API: ${path}`);
  });
  expect(
    await auth.run(
      "Waiting",
      "Logged in",
      (signal) => loginWithPasskey(signal),
    ),
  ).toBe(false);
  expect(auth.session).toEqual({ kind: "error" });
  expect(auth.status).toBe("");
  expect(auth.actor).toBeNull();
  expect(auth.busy).toBe(false);
});

for (const rejected of [false, true]) {
  test(`dispose ignores an outstanding whoami ${rejected ? "error" : "actor"} response and rejects future work`, async () => {
    const { fetcher } = boundary();
    const response = gate<Response>();
    fetcher.mockReturnValueOnce(response.promise);
    const auth = session();
    const refresh = auth.refresh();
    auth.dispose();
    response.resolve(
      rejected ? new Response(null, { status: 503 }) : whoami(true),
    );
    await refresh;
    expect(auth.session).toEqual({ kind: "loading" });
    expect(auth.actor).toBeNull();
    await auth.refresh();
    const operation = vi.fn(async () => {});
    expect(await auth.run("Waiting", "Logged in", operation)).toBe(false);
    expect(operation).not.toHaveBeenCalled();
    expect(fetcher).toHaveBeenCalledOnce();
  });
}

test("dispose aborts WebAuthn without accepting late credentials, while shared session is rechecked", async () => {
  const { fetcher, get } = boundary();
  const auth = session();
  await auth.refresh();
  const credential = gate<Credential | null>();
  const started = gate<AbortSignal>();
  get.mockImplementationOnce((options) => {
    started.resolve(options!.signal!);
    return credential.promise;
  });
  const dispatch = vi.spyOn(window, "dispatchEvent");
  const run = auth.run(
    "Waiting",
    "Logged in",
    (signal) => loginWithPasskey(signal),
  );
  const signal = await started.promise;
  expect(signal.aborted).toBe(false);
  auth.dispose();
  expect(signal.aborted).toBe(true);
  credential.resolve(credentialFixture(true));
  expect(await run).toBe(false);
  expect(auth.session).toEqual({
    kind: "ready",
    actor: null,
    session: { kind: "anonymous" },
  });
  expect(auth.status).toBe("Waiting");
  expect(auth.error).toBe("");
  expect(paths(fetcher)).toEqual([
    "/api/auth/whoami",
    "/api/auth/passkeys/login/options",
    "/api/auth/whoami",
  ]);
  expect(
    dispatch.mock.calls.filter(([event]) =>
      event.type === SESSION_CHANGED_EVENT
    ),
  ).toEqual([]);
});

test("dispose ignores a completion response even when the HTTP boundary does not honor abort", async () => {
  const { fetcher } = boundary();
  const auth = session();
  await auth.refresh();
  const response = gate<Response>();
  const started = gate<AbortSignal>();
  fetcher.mockImplementation(async (input, init) => {
    const path = String(input);
    if (path === "/api/auth/passkeys/login/options") {
      return Response.json(loginOptions);
    }
    if (path === "/api/auth/passkeys/login/complete") {
      started.resolve(init!.signal!);
      return await response.promise;
    }
    throw new Error(`Unexpected API: ${path}`);
  });
  const dispatch = vi.spyOn(window, "dispatchEvent");
  const run = auth.run(
    "Waiting",
    "Logged in",
    (signal) => loginWithPasskey(signal),
  );
  const signal = await started.promise;
  auth.dispose();
  expect(signal.aborted).toBe(true);
  response.resolve(Response.json({ user: authUser }));
  expect(await run).toBe(false);
  expect(auth.actor).toBeNull();
  expect(auth.status).toBe("Waiting");
  expect(auth.error).toBe("");
  expect(paths(fetcher)).toEqual([
    "/api/auth/whoami",
    "/api/auth/passkeys/login/options",
    "/api/auth/passkeys/login/complete",
    "/api/auth/whoami",
  ]);
  expect(
    dispatch.mock.calls.filter(([event]) =>
      event.type === SESSION_CHANGED_EVENT
    ),
  ).toEqual([]);
});

test("dispose during the post-operation whoami prevents its late actor from becoming readiness", async () => {
  const { fetcher } = boundary();
  const auth = session();
  await auth.refresh();
  const response = gate<Response>();
  const started = gate<void>();
  fetcher.mockImplementation(async (input) => {
    const path = String(input);
    if (path === "/api/auth/passkeys/login/options") {
      return Response.json(loginOptions);
    }
    if (path === "/api/auth/passkeys/login/complete") {
      return Response.json({ user: authUser });
    }
    if (path === "/api/auth/whoami") {
      started.resolve();
      return await response.promise;
    }
    throw new Error(`Unexpected API: ${path}`);
  });
  const run = auth.run(
    "Waiting",
    "Logged in",
    (signal) => loginWithPasskey(signal),
  );
  await started.promise;
  expect(auth.session.kind).toBe("ready");
  expect(auth.busy).toBe(true);
  auth.dispose();
  response.resolve(whoami(true));
  expect(await run).toBe(false);
  expect(auth.session.kind).toBe("ready");
  expect(auth.actor).toBeNull();
});

for (const newer of ["logout", "another-actor"] as const) {
  test(`confirmation superseded by ${newer} cannot report success or navigate using the old return URL`, async () => {
    const { fetcher } = boundary();
    const auth = session();
    await auth.refresh();
    const confirming = gate<Response>();
    const started = gate<void>();
    const request = fetcher.getMockImplementation()!;
    fetcher.mockImplementation((input, init) => {
      if (String(input) === "/api/auth/whoami") {
        started.resolve();
        return confirming.promise;
      }
      return request(input, init);
    });
    const changed = vi.fn();
    window.addEventListener(SESSION_CHANGED_EVENT, changed);
    const navigate = vi.fn();
    try {
      const run = auth.run("Waiting", "Logged in", loginWithPasskey).then(
        (confirmed) => {
          if (confirmed && auth.actor) navigate("/account?old=return#entry");
          return confirmed;
        },
      );
      await started.promise;
      sessionMutation(newer === "logout" ? "logout" : "authentication");
      fetcher.mockResolvedValueOnce(
        newer === "logout" ? whoami(false) : Response.json({
          actor: {
            ...authUser,
            user_id: "other-user",
            account_id: "other-account",
            handle: "bob",
            auth_method: "browser_session",
          },
          session: {
            kind: "authenticated",
            expires_at: "2099-01-01T00:00:00Z",
          },
        }),
      );
      await browserSession.refresh();
      confirming.resolve(whoami(true));
      expect(await run).toBe(false);
      expect(auth.actor?.handle ?? null).toBe(
        newer === "logout" ? null : "bob",
      );
      expect(auth.status).toBe("");
      expect(auth.busy).toBe(false);
      expect(changed).not.toHaveBeenCalled();
      expect(navigate).not.toHaveBeenCalled();
    } finally {
      window.removeEventListener(SESSION_CHANGED_EVENT, changed);
    }
  });
}

for (const purpose of ["login", "addition"] as const) {
  test(`${purpose} null credential reports cancellation and never sends completion`, async () => {
    const { fetcher, get, create } = boundary(purpose === "addition");
    const auth = session();
    await auth.refresh();
    (purpose === "login" ? get : create).mockResolvedValueOnce(null);
    expect(
      await auth.run(
        "Waiting",
        "Confirmed",
        (signal) =>
          purpose === "login" ? loginWithPasskey(signal) : addPasskey(signal),
      ),
    ).toBe(false);
    expect(auth.error).toMatch(/cancelled or timed out/);
    expect(paths(fetcher).some((path) => path.endsWith("/complete"))).toBe(
      false,
    );
  });
}
