// @vitest-environment happy-dom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/svelte";
import { browserSession } from "#lib/workspace/auth/session-state.ts";
import { afterEach, expect, test, vi } from "vitest";
import { tick } from "svelte";
import { goto } from "$app/navigation";
import Account from "./+page.svelte";
import Login from "../login/+page.svelte";
import Register from "../register/+page.svelte";
import Device from "../login/device/+page.svelte";
import {
  authUser,
  credentialFixture,
  loginOptions,
  registrationOptions,
} from "#lib/workspace/auth/api.test-fixtures.ts";

const page = vi.hoisted(() => ({ url: new URL("https://example.test/login") }));
vi.mock("$app/navigation", () => ({ goto: vi.fn(async () => {}) }));
vi.mock("$app/state", () => ({ page }));

afterEach(() => {
  cleanup();
  browserSession.invalidate();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
  page.url = new URL("https://example.test/login");
});

function gate<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

type Policy = boolean | "error" | "loading";
// 各routeのUI→本番HTTP/WebAuthn変換を接続し、fetchとcredential境界だけを代替する。
// Backendの署名検証・認可・永続化はこのsuiteの保証対象ではない。
function boundary({
  authenticated = false,
  completeStatus = 200,
  policy = true as Policy,
} = {}) {
  const state = { authenticated, completeStatus, policy, whoamiStatus: 200 };
  const policyResponse = gate<Response>();
  const create = vi.fn<CredentialsContainer["create"]>()
    .mockResolvedValue(credentialFixture());
  const get = vi.fn<CredentialsContainer["get"]>()
    .mockResolvedValue(credentialFixture(true));
  vi.stubGlobal("navigator", { credentials: { create, get } });
  const fetcher = vi.fn<typeof fetch>().mockImplementation(async (input) => {
    const path = String(input);
    if (path === "/api/auth/whoami") {
      if (state.whoamiStatus !== 200) {
        return new Response(null, { status: state.whoamiStatus });
      }
      return Response.json({
        session: state.authenticated
          ? { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" }
          : { kind: "anonymous" },
        actor: state.authenticated
          ? { ...authUser, auth_method: "browser_session" }
          : null,
      });
    }
    if (path === "/api/auth/registration-policy") {
      if (state.policy === "loading") return await policyResponse.promise;
      if (state.policy === "error") return new Response(null, { status: 503 });
      return Response.json({ self_registration_enabled: state.policy });
    }
    if (path === "/api/auth/passkeys/login/options") {
      return Response.json(loginOptions);
    }
    if (
      /^\/api\/auth\/passkeys\/(registration|addition)\/options$/.test(path)
    ) {
      return Response.json(registrationOptions);
    }
    if (
      /^\/api\/auth\/passkeys\/(login|registration|addition)\/complete$/.test(
        path,
      )
    ) {
      if (state.completeStatus !== 200) {
        if (state.completeStatus === 403 && path.includes("/registration/")) {
          state.policy = false;
        }
        if (state.completeStatus === 401) state.authenticated = false;
        return new Response(null, { status: state.completeStatus });
      }
      state.authenticated = true;
      return Response.json({ user: authUser });
    }
    if (path === "/api/auth/logout") {
      state.authenticated = false;
      return Response.json({ status: "logged_out" });
    }
    if (path === "/api/auth/device-login/approve") {
      if (state.completeStatus !== 200) {
        if (state.completeStatus === 401) state.authenticated = false;
        return new Response(null, { status: state.completeStatus });
      }
      return Response.json({ status: "approved", user: authUser });
    }
    throw new Error(`Unexpected API: ${path}`);
  });
  vi.stubGlobal("fetch", fetcher);
  return { fetcher, create, get, state, policyResponse };
}

function posted(fetcher: ReturnType<typeof boundary>["fetcher"]) {
  return fetcher.mock.calls.filter(([, init]) => init?.method === "POST").map(
    ([path, init]) => ({
      path: String(path),
      body: JSON.parse(String(init!.body)),
      credentials: init!.credentials,
    }),
  );
}

function apiPaths(fetcher: ReturnType<typeof boundary>["fetcher"]) {
  return fetcher.mock.calls.map(([path]) => String(path));
}

async function enterIdentity(registering = false) {
  if (!registering) {
    await screen.findByRole("button", { name: "Log in with passkey" });
    return;
  }
  await fireEvent.input(await screen.findByLabelText("Account ID (handle)"), {
    target: { value: " alice " },
  });
  if (registering) {
    await fireEvent.input(screen.getByLabelText("Display name"), {
      target: { value: " Alice " },
    });
  }
}

async function submit(name: string) {
  await fireEvent.submit(screen.getByRole("button", { name }).closest("form")!);
}

test("normal login needs no account ID and links separate registration and code activation", async () => {
  const { fetcher } = boundary({ policy: true });
  page.url = new URL("https://example.test/login?returnTo=%2Faccount");
  render(Login);
  await screen.findByRole("button", { name: "Log in with passkey" });
  expect(
    (screen.getByText("Use an older key with account ID").closest(
      "details",
    ) as HTMLDetailsElement).open,
  ).toBe(false);
  expect(screen.queryByLabelText("Display name")).toBeNull();
  expect(screen.queryByRole("button", { name: "Register new account" }))
    .toBeNull();
  expect(
    (await screen.findByRole("link", { name: "Create a new account" }))
      .getAttribute(
        "href",
      ),
  )
    .toBe("/register?returnTo=%2Faccount");
  expect(
    screen.getByRole("link", { name: "Activate an account with a code" })
      .getAttribute("href"),
  )
    .toBe("/activate");
  expect(
    (screen.getByRole("button", {
      name: "Log in with passkey",
    }) as HTMLButtonElement).disabled,
  )
    .toBe(false);
  await submit("Log in with passkey");
  await waitFor(() => expect(posted(fetcher)).toHaveLength(2));
  expect(posted(fetcher)[0].body).toEqual({
    browser_origin: window.location.origin,
  });
  expect(apiPaths(fetcher)).toContain("/api/auth/registration-policy");
});

for (
  const [returnTo, expected] of [
    [null, "/dashboard"],
    ["/account", "/account"],
    [
      "/w/project/tickets?ignored=1#fragment",
      "/w/project/tickets?ignored=1#fragment",
    ],
    [
      "/login/device?code=abcd-EFGH&untrusted=1",
      "/login/device?user_code=abcd-EFGH",
    ],
    ["https://evil.test/", "/dashboard"],
    ["//evil.test/account", "/dashboard"],
    ["/\\evil.test/account", "/dashboard"],
    ["/%2f%2fevil.test", "/dashboard"],
    ["/login?returnTo=/login", "/dashboard"],
    ["/register", "/dashboard"],
    ["/activate", "/dashboard"],
  ] as const
) {
  test(`login confirms whoami before navigating safely for returnTo=${returnTo}`, async () => {
    const { fetcher, get, create } = boundary();
    page.url = new URL("https://example.test/login");
    if (returnTo !== null) page.url.searchParams.set("returnTo", returnTo);
    render(Login);
    await enterIdentity();
    await submit("Log in with passkey");
    await waitFor(() =>
      expect(goto).toHaveBeenCalledWith(expected, { replaceState: true })
    );
    const requests = posted(fetcher);
    expect(requests.map(({ path }) => path)).toEqual([
      "/api/auth/passkeys/login/options",
      "/api/auth/passkeys/login/complete",
    ]);
    expect(requests[0].body).toEqual({
      browser_origin: window.location.origin,
    });
    expect(requests[1].body.challenge_id).toBe(loginOptions.challenge_id);
    expect(get).toHaveBeenCalledOnce();
    expect(create).not.toHaveBeenCalled();
    expect(apiPaths(fetcher).filter((path) => path === "/api/auth/whoami"))
      .toHaveLength(2);
    expect(apiPaths(fetcher)).not.toContain("/api/auth/device-login/approve");
  });
}

test("successful login completion without confirmed whoami never navigates", async () => {
  const { state, get } = boundary();
  render(Login);
  await enterIdentity();
  get.mockImplementationOnce(async () => {
    state.whoamiStatus = 503;
    return credentialFixture(true);
  });
  await submit("Log in with passkey");
  await screen.findByText(/Your session could not be loaded/);
  expect(goto).not.toHaveBeenCalled();
  expect(screen.queryByRole("button", { name: "Log in with passkey" }))
    .toBeNull();
});

test("register requires a separate ID and display name and uses only new-account endpoints", async () => {
  const { fetcher, create, get } = boundary();
  render(Register);
  await screen.findByLabelText("Display name");
  expect(
    (screen.getByRole("button", {
      name: "Register new account",
    }) as HTMLButtonElement).disabled,
  )
    .toBe(true);
  await fireEvent.input(screen.getByLabelText("Account ID (handle)"), {
    target: { value: " alice " },
  });
  expect(
    (screen.getByRole("button", {
      name: "Register new account",
    }) as HTMLButtonElement).disabled,
  )
    .toBe(true);
  expect(screen.queryByRole("button", { name: "Log in with passkey" }))
    .toBeNull();
  await fireEvent.input(screen.getByLabelText("Display name"), {
    target: { value: " Alice " },
  });
  await submit("Register new account");
  await waitFor(() =>
    expect(goto).toHaveBeenCalledWith("/dashboard", { replaceState: true })
  );
  const requests = posted(fetcher);
  expect(requests.map(({ path }) => path)).toEqual([
    "/api/auth/passkeys/registration/options",
    "/api/auth/passkeys/registration/complete",
  ]);
  expect(requests[0].body).toEqual({
    handle: "alice",
    display_name: "Alice",
    browser_origin: window.location.origin,
  });
  expect(requests[1].body.challenge_id).toBe(registrationOptions.challenge_id);
  expect(create).toHaveBeenCalledOnce();
  expect(get).not.toHaveBeenCalled();
});

for (const policy of [false, "error", "loading"] as const) {
  test(`register policy ${policy} prevents registration but preserves login and activation links`, async () => {
    const { fetcher, create, policyResponse } = boundary({ policy });
    render(Register);
    await screen.findByRole("heading", { name: "Create account" });
    if (policy === false) {
      await screen.findByText(/New account registration is disabled/);
    }
    if (policy === "error") {
      await screen.findByRole("button", {
        name: "Retry registration availability",
      });
    }
    if (policy === "loading") {
      await screen.findByText("Checking registration availability…");
    }
    await waitFor(() =>
      expect(apiPaths(fetcher)).toContain("/api/auth/whoami")
    );
    expect(screen.queryByLabelText("Account ID (handle)")).toBeNull();
    expect(screen.queryByRole("button", { name: "Register new account" }))
      .toBeNull();
    expect(
      screen.getByRole("link", { name: "Log in with an existing account" })
        .getAttribute("href"),
    )
      .toBe("/login?returnTo=%2Fdashboard");
    expect(
      screen.getByRole("link", { name: "Activate an account with a code" })
        .getAttribute("href"),
    )
      .toBe("/activate");
    expect(posted(fetcher)).toEqual([]);
    expect(create).not.toHaveBeenCalled();
    if (policy === "loading") {
      policyResponse.resolve(
        Response.json({ self_registration_enabled: false }),
      );
      await screen.findByText(/New account registration is disabled/);
    }
  });
}

test("policy disabled during WebAuthn rejects registration, reloads policy and never falls back to addition", async () => {
  const { fetcher, create, state } = boundary();
  const credential = gate<Credential | null>();
  create.mockReturnValueOnce(credential.promise);
  render(Register);
  await enterIdentity(true);
  await submit("Register new account");
  await waitFor(() => expect(create).toHaveBeenCalledOnce());
  state.completeStatus = 403;
  credential.resolve(credentialFixture());
  await screen.findByText("Auth request failed (403).");
  await screen.findByText(/New account registration is disabled/);
  expect(screen.queryByRole("button", { name: "Register new account" }))
    .toBeNull();
  expect(screen.queryByText("New account registered.")).toBeNull();
  expect(posted(fetcher).map(({ path }) => path)).toEqual([
    "/api/auth/passkeys/registration/options",
    "/api/auth/passkeys/registration/complete",
  ]);
  expect(goto).not.toHaveBeenCalled();
});

test("account displays the current identity without editable target fields and adds only to self", async () => {
  const { fetcher, create, get } = boundary({
    authenticated: true,
    policy: false,
  });
  render(Account);
  await screen.findByRole("button", { name: "Add passkey to alice" });
  expect(screen.getByText("alice")).toBeTruthy();
  expect(screen.getByText("Alice")).toBeTruthy();
  expect(screen.queryAllByRole("textbox")).toEqual([]);
  expect(screen.queryByRole("button", { name: "Log in with passkey" }))
    .toBeNull();
  expect(screen.queryByRole("button", { name: "Register new account" }))
    .toBeNull();
  await fireEvent.click(
    screen.getByRole("button", { name: "Add passkey to alice" }),
  );
  await screen.findByText("Passkey added to your account.");
  await waitFor(() =>
    expect(
      (screen.getByRole("button", { name: "Log out" }) as HTMLButtonElement)
        .disabled,
    ).toBe(false)
  );
  const requests = posted(fetcher);
  expect(requests.map(({ path }) => path)).toEqual([
    "/api/auth/passkeys/addition/options",
    "/api/auth/passkeys/addition/complete",
  ]);
  expect(requests[0].body).toEqual({ browser_origin: window.location.origin });
  expect(Object.keys(requests[1].body).sort()).toEqual([
    "challenge_id",
    "credential",
  ]);
  expect(requests[1].body.challenge_id).toBe(registrationOptions.challenge_id);
  expect(requests.map(({ credentials }) => credentials)).toEqual([
    "same-origin",
    "same-origin",
  ]);
  expect(apiPaths(fetcher)).not.toContain("/api/auth/registration-policy");
  expect(create).toHaveBeenCalledOnce();
  expect(get).not.toHaveBeenCalled();
  expect(goto).not.toHaveBeenCalled();
});

test("account fragment leaves anonymous navigation to the root gate and has no credential inputs", async () => {
  const { fetcher } = boundary();
  render(Account);
  await screen.findByRole("link", { name: "Log in to your account" });
  expect(goto).not.toHaveBeenCalled();
  expect(
    screen.getByRole("link", { name: "Log in to your account" }).getAttribute(
      "href",
    ),
  )
    .toBe("/login?returnTo=%2Faccount");
  expect(screen.queryAllByRole("textbox")).toEqual([]);
  expect(posted(fetcher)).toEqual([]);
});

for (
  const [route, Component] of [
    ["login", Login],
    ["register", Register],
    ["account", Account],
    ["device", Device],
  ] as const
) {
  test(`${route} keeps loading and unknown whoami separate from an anonymous session`, async () => {
    const { fetcher } = boundary();
    const response = gate<Response>();
    fetcher.mockReturnValueOnce(response.promise);
    if (Component === Login) render(Login);
    else if (Component === Register) render(Register);
    else if (Component === Account) render(Account);
    else render(Device);
    await screen.findByText("Loading session…");
    expect(
      screen.queryByRole("button", {
        name:
          /Log in with passkey|Register new account|Add passkey|Approve device login|Log out/,
      }),
    ).toBeNull();
    expect(goto).not.toHaveBeenCalled();
    response.resolve(new Response(null, { status: 503 }));
    await screen.findByText(/Your session could not be loaded/);
    expect(screen.queryByText("No browser session is active.")).toBeNull();
    expect(
      screen.queryByRole("button", {
        name:
          /Log in with passkey|Register new account|Add passkey|Approve device login|Log out/,
      }),
    ).toBeNull();
    expect(posted(fetcher)).toEqual([]);
    expect(goto).not.toHaveBeenCalled();
  });
}

test("unknown account session shows retry instead of treating errors as anonymous or redirecting", async () => {
  const { state, fetcher } = boundary();
  state.whoamiStatus = 503;
  render(Account);
  await screen.findByText(/Your session could not be loaded/);
  expect(screen.queryByText("No browser session is active.")).toBeNull();
  expect(screen.queryByRole("button", { name: /Add passkey/ })).toBeNull();
  expect(goto).not.toHaveBeenCalled();
  state.whoamiStatus = 200;
  state.authenticated = true;
  await fireEvent.click(screen.getByRole("button", { name: "Retry session" }));
  await screen.findByRole("button", { name: "Add passkey to alice" });
  expect(posted(fetcher)).toEqual([]);
  expect(goto).not.toHaveBeenCalled();
});

test("logout confirms the server session is cleared and returns to the public home", async () => {
  const { fetcher } = boundary({ authenticated: true });
  render(Account);
  await fireEvent.click(await screen.findByRole("button", { name: "Log out" }));
  await waitFor(() =>
    expect(goto).toHaveBeenCalledWith("/", {
      replaceState: true,
    })
  );
  expect(posted(fetcher)).toEqual([
    { path: "/api/auth/logout", body: {}, credentials: "same-origin" },
  ]);
  expect(apiPaths(fetcher)).toEqual([
    "/api/auth/whoami",
    "/api/auth/logout",
    "/api/auth/whoami",
  ]);
  expect(screen.queryByRole("button", { name: /Add passkey/ })).toBeNull();
});

test("expired addition revokes stale identity without silently retrying as registration", async () => {
  const { fetcher } = boundary({ authenticated: true, completeStatus: 401 });
  render(Account);
  await fireEvent.click(
    await screen.findByRole("button", { name: "Add passkey to alice" }),
  );
  await screen.findByText(/Authentication request was rejected/);
  await screen.findByRole("link", { name: "Log in to your account" });
  expect(screen.queryByRole("button", { name: /Add passkey/ })).toBeNull();
  expect(screen.queryByText("Passkey added to your account.")).toBeNull();
  expect(posted(fetcher).map(({ path }) => path)).toEqual([
    "/api/auth/passkeys/addition/options",
    "/api/auth/passkeys/addition/complete",
  ]);
});

test("pending login disables inputs and rejects duplicate form submits until the credential completes", async () => {
  const { fetcher, get } = boundary();
  const credential = gate<Credential | null>();
  get.mockReturnValueOnce(credential.promise);
  render(Login);
  await enterIdentity();
  await submit("Log in with passkey");
  await waitFor(() => expect(get).toHaveBeenCalledOnce());
  expect(
    (screen.getByLabelText("Account ID (handle)") as HTMLInputElement).disabled,
  ).toBe(true);
  expect(
    (screen.getByRole("button", {
      name: "Log in with passkey",
    }) as HTMLButtonElement).disabled,
  ).toBe(true);
  await submit("Log in with passkey");
  expect(posted(fetcher).map(({ path }) => path)).toEqual([
    "/api/auth/passkeys/login/options",
  ]);
  expect(goto).not.toHaveBeenCalled();
  credential.resolve(credentialFixture(true));
  await waitFor(() => expect(goto).toHaveBeenCalledOnce());
  expect(get).toHaveBeenCalledOnce();
  expect(posted(fetcher)).toHaveLength(2);
});

test("WebAuthn cancellation is visible, sends no completion, and allows an explicit retry", async () => {
  const { fetcher, get } = boundary();
  get.mockRejectedValueOnce(
    new DOMException("sensitive browser detail", "NotAllowedError"),
  );
  render(Login);
  await enterIdentity();
  await submit("Log in with passkey");
  await screen.findByText(
    "Passkey operation was cancelled or timed out, or no matching key was found. Completion is not confirmed. Use a supported device and try again.",
  );
  await waitFor(() =>
    expect(
      (screen.getByRole("button", {
        name: "Log in with passkey",
      }) as HTMLButtonElement).disabled,
    ).toBe(false)
  );
  expect(screen.queryByText(/sensitive browser detail/)).toBeNull();
  expect(posted(fetcher).map(({ path }) => path)).toEqual([
    "/api/auth/passkeys/login/options",
  ]);
  expect(goto).not.toHaveBeenCalled();
  await submit("Log in with passkey");
  await waitFor(() => expect(goto).toHaveBeenCalledOnce());
  expect(get).toHaveBeenCalledTimes(2);
});

test("unmount aborts a pending WebAuthn login and ignores a credential that arrives afterwards", async () => {
  const { fetcher, get } = boundary();
  const credential = gate<Credential | null>();
  get.mockReturnValueOnce(credential.promise);
  const view = render(Login);
  await enterIdentity();
  await submit("Log in with passkey");
  await waitFor(() => expect(get).toHaveBeenCalledOnce());
  const signal = get.mock.calls[0][0]?.signal;
  expect(signal?.aborted).toBe(false);
  view.unmount();
  expect(signal?.aborted).toBe(true);
  credential.resolve(credentialFixture(true));
  await credential.promise;
  await tick();
  expect(posted(fetcher).map(({ path }) => path)).toEqual([
    "/api/auth/passkeys/login/options",
  ]);
  expect(goto).not.toHaveBeenCalled();
});

test("device login round trip preserves the code and still requires an explicit approval", async () => {
  const { fetcher } = boundary();
  page.url = new URL("https://example.test/login/device?user_code=abcd-efgh");
  const device = render(Device);
  const login = await screen.findByRole("link", { name: "Log in to continue" });
  expect(screen.queryByRole("button", { name: "Approve device login" }))
    .toBeNull();
  expect(posted(fetcher)).toEqual([]);
  const destination = new URL(login.getAttribute("href")!, page.url);
  expect(destination.pathname).toBe("/login");
  expect(destination.searchParams.get("returnTo")).toBe(
    "/login/device?user_code=abcd-efgh",
  );
  device.unmount();
  page.url = destination;
  const signIn = render(Login);
  await enterIdentity();
  await submit("Log in with passkey");
  await waitFor(() =>
    expect(goto).toHaveBeenCalledWith("/login/device?user_code=abcd-efgh", {
      replaceState: true,
    })
  );
  expect(apiPaths(fetcher)).not.toContain("/api/auth/device-login/approve");
  signIn.unmount();
  page.url = new URL("https://example.test/login/device?user_code=abcd-efgh");
  render(Device);
  const approve = await screen.findByRole("button", {
    name: "Approve device login",
  });
  expect((screen.getByLabelText("User code") as HTMLInputElement).value).toBe(
    "abcd-efgh",
  );
  expect(apiPaths(fetcher)).not.toContain("/api/auth/device-login/approve");
  await fireEvent.submit(approve.closest("form")!);
  await screen.findByText("Device login approved. You can return to the CLI.");
  await waitFor(() =>
    expect(screen.queryByRole("button", { name: "Approve device login" }))
      .toBeNull()
  );
  expect(
    posted(fetcher).filter(({ path }) =>
      path === "/api/auth/device-login/approve"
    ),
  )
    .toEqual([{
      path: "/api/auth/device-login/approve",
      body: { user_code: "ABCD-EFGH" },
      credentials: "same-origin",
    }]);
  await fireEvent.submit(screen.getByLabelText("User code").closest("form")!);
  expect(
    posted(fetcher).filter(({ path }) =>
      path === "/api/auth/device-login/approve"
    ),
  ).toHaveLength(1);
});

test("device approval locks its code and rejects a second submit while the server response is pending", async () => {
  const { fetcher } = boundary({ authenticated: true });
  page.url = new URL("https://example.test/login/device?user_code=abcd-efgh");
  render(Device);
  const approve = await screen.findByRole("button", {
    name: "Approve device login",
  });
  const response = gate<Response>();
  fetcher.mockReturnValueOnce(response.promise);
  const form = approve.closest("form")!;
  await fireEvent.submit(form);
  await screen.findByText("Approving device login…");
  expect((approve as HTMLButtonElement).disabled).toBe(true);
  expect((screen.getByLabelText("User code") as HTMLInputElement).disabled)
    .toBe(true);
  await fireEvent.submit(form);
  expect(posted(fetcher).map(({ path }) => path)).toEqual([
    "/api/auth/device-login/approve",
  ]);
  response.resolve(Response.json({ status: "approved", user: authUser }));
  await screen.findByText("Device login approved. You can return to the CLI.");
  expect(posted(fetcher)).toHaveLength(1);
});

for (const status of [401, 410]) {
  test(`device approval ${status} is visible and never reports approval`, async () => {
    const { fetcher } = boundary({
      authenticated: true,
      completeStatus: status,
    });
    page.url = new URL("https://example.test/login/device?code=abcd-efgh");
    render(Device);
    await fireEvent.submit(
      (await screen.findByRole("button", { name: "Approve device login" }))
        .closest("form")!,
    );
    await screen.findByText(
      status === 401
        ? /Authentication request was rejected/
        : /The request or authentication challenge is invalid or expired/,
    );
    expect(
      screen.queryByText("Device login approved. You can return to the CLI."),
    ).toBeNull();
    expect(posted(fetcher)).toHaveLength(1);
    expect(goto).not.toHaveBeenCalled();
  });
}

test("explicit older-key login alone sends the trimmed account ID", async () => {
  const { fetcher } = boundary();
  render(Login);
  await screen.findByRole("button", { name: "Log in with passkey" });
  await fireEvent.click(screen.getByText("Use an older key with account ID"));
  await fireEvent.input(screen.getByLabelText("Account ID (handle)"), {
    target: { value: " alice " },
  });
  await submit("Log in with older key");
  await waitFor(() => expect(goto).toHaveBeenCalled());
  expect(posted(fetcher)[0].body).toEqual({
    handle: "alice",
    browser_origin: window.location.origin,
  });
});

test("compatibility login keeps its selected form and ID after cancellation and session refresh", async () => {
  const { fetcher, get } = boundary();
  get.mockRejectedValueOnce(new DOMException("cancelled", "NotAllowedError"));
  render(Login);
  await screen.findByRole("button", { name: "Log in with passkey" });
  const disclosure = screen.getByText("Use an older key with account ID")
    .closest("details") as HTMLDetailsElement;
  disclosure.open = true;
  await fireEvent(disclosure, new Event("toggle"));
  await fireEvent.input(screen.getByLabelText("Account ID (handle)"), {
    target: { value: "alice" },
  });
  await submit("Log in with older key");
  await screen.findByText(/cancelled or timed out/);
  await screen.findByText("Use an older key with account ID");
  expect(
    (screen.getByText("Use an older key with account ID").closest(
      "details",
    ) as HTMLDetailsElement).open,
  ).toBe(true);
  expect(
    (screen.getByLabelText("Account ID (handle)") as HTMLInputElement).value,
  ).toBe("alice");
  expect(posted(fetcher).map(({ path }) => path)).toEqual([
    "/api/auth/passkeys/login/options",
  ]);
});
