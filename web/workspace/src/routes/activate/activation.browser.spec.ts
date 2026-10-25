// @vitest-environment happy-dom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import {
  browserSession,
  sessionMutation,
} from "#lib/workspace/auth/session-state.ts";
import { SESSION_CHANGED_EVENT } from "#lib/workspace/auth/hosting.ts";
import Activate from "./+page.svelte";
import {
  authUser,
  credentialFixture,
  registrationOptions,
} from "#lib/workspace/auth/api.test-fixtures.ts";
afterEach(() => {
  cleanup();
  browserSession.invalidate();
  vi.unstubAllGlobals();
});
function gate<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}
function confirmedSession() {
  return {
    actor: { ...authUser, auth_method: "browser_session" },
    authentication_reference: "fixture-authentication",
    session: { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" },
  };
}
function boundary(
  mode:
    | "success"
    | "pending"
    | "cancel"
    | "unknown"
    | "anonymous"
    | "bob"
    | "reference-mismatch"
    | "unavailable",
) {
  Object.defineProperty(window, "isSecureContext", {
    configurable: true,
    value: true,
  });
  const create = vi.fn(async () => {
    if (mode === "pending") return await new Promise<Credential>(() => {});
    if (mode === "cancel") throw new Error("private diagnostic");
    return credentialFixture();
  });
  vi.stubGlobal("navigator", { credentials: { create } });
  const fetcher = vi.fn<typeof fetch>(async (input) => {
    if (String(input) === "/api/auth/activation/options") {
      return Response.json(registrationOptions);
    }
    if (String(input) === "/api/auth/activation/complete") {
      if (mode === "unknown") throw new Error("private diagnostic");
      return Response.json({
        user: authUser,
        authentication_reference: "fixture-authentication",
      });
    }
    if (String(input) === "/api/auth/whoami") {
      if (mode === "unavailable") {
        return new Response("private diagnostic", { status: 503 });
      }
      if (mode === "anonymous") {
        return Response.json({
          actor: null,
          authentication_reference: null,
          session: { kind: "anonymous" },
        });
      }
      const confirmation = confirmedSession();
      if (mode === "bob") {
        confirmation.actor = {
          ...confirmation.actor,
          user_id: "user-bob",
          account_id: "account-bob",
          handle: "bob",
          display_name: "Bob",
        };
      }
      if (mode === "reference-mismatch") {
        confirmation.authentication_reference = "another-authentication";
      }
      return Response.json(confirmation);
    }
    throw new Error("Unexpected endpoint");
  });
  vi.stubGlobal("fetch", fetcher);
  return { fetcher, create };
}
async function submit() {
  await fireEvent.input(screen.getByLabelText("Account ID (handle)"), {
    target: { value: " ALICE " },
  });
  await fireEvent.input(screen.getByLabelText("Activation code"), {
    target: { value: "synthetic-input-only" },
  });
  await fireEvent.click(
    screen.getByRole("button", { name: "Register Passkey and activate" }),
  );
}
test("activation page shows signed in only after complete confirmation and clears code before browser prompt", async () => {
  const { fetcher } = boundary("success");
  const confirmation = gate<Response>();
  const started = gate<void>();
  const original = fetcher.getMockImplementation()!;
  fetcher.mockImplementation((input, init) => {
    if (String(input) === "/api/auth/whoami") {
      started.resolve();
      return confirmation.promise;
    }
    return original(input, init);
  });
  render(Activate);
  await submit();
  await started.promise;
  expect(screen.queryByText(/You are signed in/)).toBeNull();
  expect(screen.queryByRole("link", { name: "Open Account" })).toBeNull();
  expect((screen.getByLabelText("Activation code") as HTMLInputElement).value)
    .toBe("");
  confirmation.resolve(Response.json(confirmedSession()));
  await screen.findByText(/registration and activation confirmed/);
  expect(fetcher.mock.calls.map(([input]) => input)).toEqual([
    "/api/auth/activation/options",
    "/api/auth/activation/complete",
    "/api/auth/whoami",
  ]);
  expect(screen.queryByLabelText("Activation code")).toBeNull();
  expect(JSON.parse(String(fetcher.mock.calls[0][1]?.body)).handle).toBe(
    "alice",
  );
});
test("pending ceremony never exposes activation or workspace actions and explicit cancel clears the input", async () => {
  const { fetcher } = boundary("pending");
  render(Activate);
  await submit();
  await screen.findByText(/activation has not been confirmed/);
  expect((screen.getByLabelText("Activation code") as HTMLInputElement).value)
    .toBe("");
  expect(screen.queryByText(/You are signed in/)).toBeNull();
  await fireEvent.click(
    screen.getByRole("button", { name: "Cancel registration" }),
  );
  await screen.findByRole("alert");
  expect(fetcher.mock.calls.length).toBe(1);
  expect(
    (screen.getByLabelText("Activation code") as HTMLInputElement).disabled,
  ).toBe(false);
});
for (const mode of ["cancel", "unknown"] as const) {
  test(`${mode} does not claim active or echo diagnostics and offers login-first recovery`, async () => {
    boundary(mode);
    render(Activate);
    await submit();
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toContain("Passkey login")
    );
    expect(screen.getByRole("alert").textContent).not.toContain(
      "private diagnostic",
    );
    expect(screen.queryByText(/You are signed in/)).toBeNull();
    expect((screen.getByLabelText("Activation code") as HTMLInputElement).value)
      .toBe("");
  });
}

for (
  const mode of [
    "anonymous",
    "bob",
    "reference-mismatch",
    "unavailable",
  ] as const
) {
  test(
    "activation " + mode +
      " confirmation never claims signed in after successful completion",
    async () => {
      const { fetcher, create } = boundary(mode);
      const changed = vi.fn();
      window.addEventListener(SESSION_CHANGED_EVENT, changed);
      try {
        render(Activate);
        await submit();
        await screen.findByRole("alert");
        expect(fetcher.mock.calls.map(([input]) => input)).toEqual([
          "/api/auth/activation/options",
          "/api/auth/activation/complete",
          "/api/auth/whoami",
        ]);
        expect(create).toHaveBeenCalledOnce();
        expect(screen.getByRole("alert").textContent).toContain(
          "Passkey login",
        );
        expect(screen.getByRole("alert").textContent).not.toContain(
          "private diagnostic",
        );
        expect(screen.queryByText(/You are signed in/)).toBeNull();
        expect(screen.queryByRole("link", { name: "Open Account" })).toBeNull();
        expect(
          (screen.getByLabelText("Activation code") as HTMLInputElement).value,
        ).toBe("");
        expect(changed).not.toHaveBeenCalled();
      } finally {
        window.removeEventListener(SESSION_CHANGED_EVENT, changed);
      }
    },
  );
}

test("logout during the activation prompt fences a late credential before completion", async () => {
  const { fetcher, create } = boundary("success");
  const credential = gate<Credential>();
  create.mockReturnValueOnce(credential.promise);
  render(Activate);
  await submit();
  await waitFor(() => expect(create).toHaveBeenCalledOnce());
  sessionMutation("logout");
  credential.resolve(credentialFixture());
  await screen.findByRole("alert");
  expect(fetcher.mock.calls.map(([input]) => input)).toEqual([
    "/api/auth/activation/options",
  ]);
  expect(screen.queryByText(/You are signed in/)).toBeNull();
  expect(screen.queryByRole("link", { name: "Open Account" })).toBeNull();
});
