// @vitest-environment happy-dom
// whoami境界だけを代替し、epoch競合/再確認契機を決定的に検査する。
// cookieと実storeの一致はServer/browser結合suiteが所有する。
import { afterEach, expect, test, vi } from "vitest";
import { SessionState } from "./session-state.ts";
import type { WhoamiResponse } from "./model.ts";
import { authUser } from "./api.test-fixtures.ts";

const states: SessionState[] = [];
afterEach(() => {
  for (const state of states.splice(0)) state.dispose();
  vi.useRealTimers();
  localStorage.clear();
});
const anonymous: WhoamiResponse = {
  actor: null,
  session: { kind: "anonymous" },
};
const expired: WhoamiResponse = { actor: null, session: { kind: "expired" } };
const authenticated = (
  expires_at: string | null = "2099-01-01T00:00:00Z",
): WhoamiResponse => ({
  actor: { ...authUser, auth_method: "browser_session" },
  session: { kind: "authenticated", expires_at },
});
function gate<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}
function state(read: () => Promise<WhoamiResponse>) {
  const result = new SessionState(read);
  states.push(result);
  return result;
}

test("concurrent session checks share one authoritative read without classifying loading as anonymous", async () => {
  const response = gate<WhoamiResponse>();
  const read = vi.fn(() => response.promise);
  const session = state(read);
  const first = session.refresh();
  const second = session.refresh();
  expect(session.snapshot.kind).toBe("loading");
  expect(read).toHaveBeenCalledTimes(1);
  response.resolve(expired);
  await Promise.all([first, second]);
  expect(session.snapshot).toEqual({ kind: "ready", value: expired });
});

test("delayed expired response cannot replace a new login in the same tab", async () => {
  const older = gate<WhoamiResponse>();
  const read = vi.fn<() => Promise<WhoamiResponse>>().mockReturnValueOnce(
    older.promise,
  ).mockResolvedValue(authenticated());
  const session = state(read);
  const first = session.refresh();
  session.invalidate();
  await session.refresh();
  older.resolve(expired);
  await first;
  expect(session.snapshot).toEqual({ kind: "ready", value: authenticated() });
});

test("storage generation fences responses even before the other tab storage event is delivered", async () => {
  const older = gate<WhoamiResponse>();
  const read = vi.fn<() => Promise<WhoamiResponse>>().mockReturnValueOnce(
    older.promise,
  ).mockResolvedValue(authenticated());
  const session = state(read);
  const first = session.refresh();
  localStorage.setItem(
    "yoi.browser-auth-generation",
    JSON.stringify({ id: "new-login", reason: "authentication" }),
  );
  older.resolve(expired);
  await first;
  expect(session.snapshot).toEqual({ kind: "ready", value: authenticated() });
});

test("active deadline rechecks the server and reports failures as unknown, never inferred expiry", async () => {
  vi.useFakeTimers();
  const read = vi.fn<() => Promise<WhoamiResponse>>().mockResolvedValueOnce(
    authenticated(new Date(Date.now() + 5_000).toISOString()),
  ).mockRejectedValueOnce(new Error("network")).mockResolvedValue(expired);
  const session = state(read);
  await session.refresh();
  await vi.advanceTimersByTimeAsync(5_000);
  expect(session.snapshot).toEqual({ kind: "error" });
  await session.refresh();
  expect(session.snapshot).toEqual({ kind: "ready", value: expired });
});

test("a challenge failure is not session expiry; recheck may still be authenticated or anonymous", async () => {
  for (const value of [authenticated(), anonymous]) {
    const session = state(async () => value);
    session.invalidate();
    await session.refresh();
    expect(session.snapshot).toEqual({ kind: "ready", value });
  }
});
