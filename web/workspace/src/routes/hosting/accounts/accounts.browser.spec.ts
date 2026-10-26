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
import Accounts from "./+page.svelte";
import { authUser } from "#lib/workspace/auth/api.test-fixtures.ts";
afterEach(() => {
  cleanup();
  browserSession.invalidate();
  vi.unstubAllGlobals();
});
function boundary(
  mode: "admin" | "anonymous" | "ordinary" | "revoked" | "unknown" | "loading",
) {
  let accesses = 0;
  const fetcher = vi.fn<typeof fetch>(async (input, init) => {
    if (input === "/api/auth/whoami") {
      return Response.json({
        authentication_reference: mode !== "anonymous" ? "fixture-authentication" : null,
        session: mode !== "anonymous"
          ? { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" }
          : { kind: "anonymous" },
        actor: mode === "anonymous"
          ? null
          : { ...authUser, auth_method: "browser_session" },
      });
    }
    if (input === "/api/hosting/access") {
      accesses++;
      if (mode === "loading") return await new Promise<Response>(() => {});
      return Response.json({
        is_admin: mode !== "ordinary" && !(mode === "revoked" && accesses > 1),
      });
    }
    if (
      init?.method === "POST" &&
      String(input).startsWith("/api/hosting/accounts")
    ) {
      if (mode === "unknown") throw new Error("private diagnostic");
      return Response.json({
        user: authUser,
        code: "synthetic-output-only",
        expires_at: new Date(Date.now() + 900000).toISOString(),
      });
    }
    throw new Error("Unexpected endpoint");
  });
  vi.stubGlobal("fetch", fetcher);
  return fetcher;
}
async function issue() {
  await fireEvent.input(await screen.findByLabelText("Account ID (handle)"), {
    target: { value: " ALICE " },
  });
  await fireEvent.click(
    screen.getByRole("button", { name: "Create account and issue code" }),
  );
}
for (const mode of ["anonymous", "ordinary", "loading"] as const) {
  test(`${mode} hides account mutations before current administrator access is confirmed`, async () => {
    const fetcher = boundary(mode);
    render(Accounts);
    await screen.findByText(
      mode === "anonymous"
        ? /Sign in with a hosting administrator/
        : mode === "ordinary"
        ? "Hosting administrator access is required."
        : "Checking hosting access…",
    );
    expect(screen.queryByLabelText("Account ID (handle)")).toBeNull();
    expect(fetcher.mock.calls.some(([, init]) => init?.method === "POST")).toBe(
      false,
    );
  });
}
test("creation shows issuance only in page memory and hide clears the code", async () => {
  const fetcher = boundary("admin");
  render(Accounts);
  await issue();
  await screen.findByRole("heading", { name: "Activation code issued" });
  const post = fetcher.mock.calls.find(([, init]) => init?.method === "POST")!;
  expect(JSON.parse(String(post[1]?.body))).toEqual({ handle: "alice" });
  expect(post[0]).toBe("/api/hosting/accounts");
  await fireEvent.click(screen.getByRole("button", { name: "Hide code" }));
  expect(screen.queryByText("synthetic-output-only")).toBeNull();
  expect(screen.getByLabelText("Account ID (handle)")).toBeTruthy();
  // 実際のselect操作と再発行はproduction-shell browser suiteが確認する。
});
test("access revoked after page load prevents issuance", async () => {
  const fetcher = boundary("revoked");
  render(Accounts);
  await issue();
  await screen.findByText("Hosting administrator access is required.");
  expect(fetcher.mock.calls.some(([, init]) => init?.method === "POST")).toBe(
    false,
  );
});
test("lost issuance response never retries or echoes private diagnostics", async () => {
  const fetcher = boundary("unknown");
  render(Accounts);
  await issue();
  await waitFor(() =>
    expect(screen.getByRole("alert").textContent).toContain(
      "could not be confirmed",
    )
  );
  expect(screen.getByRole("alert").textContent).not.toContain(
    "private diagnostic",
  );
  expect(screen.queryByRole("heading", { name: "Activation code issued" }))
    .toBeNull();
  expect(
    fetcher.mock.calls.filter(([, init]) => init?.method === "POST").length,
  ).toBe(1);
});
