// @vitest-environment happy-dom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { tick } from "svelte";
import SubjectBody from "./SubjectBody.svelte";
import {
  readSubjectBodyAttempt,
  subjectBodyStorageKey,
} from "./subject-body.ts";

// component・client/parser・sessionStorageは本番実装を使い、HTTP境界だけを置き換える。
const props = {
  accountId: "account-a",
  workspaceId: "workspace",
  subjectId: "subject",
};
const owner = { runtime_id: "runtime", worker_id: "worker", generation: "17" };
function absent(subjectId = props.subjectId) {
  return {
    workspace_id: props.workspaceId,
    subject_id: subjectId,
    current: null,
    observation: { kind: "absent" },
    can_start: true,
    can_stop: true,
  };
}
function available(state = "stopped", observationToken = "observation-1") {
  return {
    ...absent(),
    current: owner,
    observation: {
      kind: "available",
      worker: {
        runtime_id: owner.runtime_id,
        worker_id: owner.worker_id,
        resource_key: "W-1",
        host_id: "host",
        label: "Body",
        workspace: {
          visibility: "workspace",
          identity: "workspace",
          workspace_id: "workspace",
        },
        implementation: { kind: "embedded", display_hint: "" },
        availability: "observed",
        state,
        worker_state: state === "stopped"
          ? null
          : { last_command_id: 1, state: { kind: "idle" } },
        restore_observation_token: observationToken,
      },
    },
  };
}
function savedStop(accountId = props.accountId) {
  return {
    accountId,
    workspaceId: props.workspaceId,
    subjectId: props.subjectId,
    action: "stop",
    request: {
      request_id: `saved-stop-${accountId}`,
      expected_current: owner,
      expected_observation_token: "saved-observation",
    },
  };
}

// Happy DOM does not update bodyUsed when the production bounded reader consumes a stream.
// Observe releaseLock instead, so late-response assertions run after actual consumption.
function afterBodyRead(response: Response): Promise<void> {
  return new Promise((resolve) => {
    const getReader = response.body!.getReader.bind(response.body);
    vi.spyOn(response.body!, "getReader").mockImplementation(() => {
      const reader = getReader();
      const release = reader.releaseLock.bind(reader);
      vi.spyOn(reader, "releaseLock").mockImplementation(() => {
        release();
        resolve();
      });
      return reader;
    });
  });
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((accept) => {
    resolve = accept;
  });
  return { promise, resolve };
}
// Happy DOM Storage is a Proxy with bound own methods; spy on a plain adapter
// rather than its unrelated Storage.prototype or non-restorable proxy properties.
beforeEach(() => {
  const storage = sessionStorage;
  vi.stubGlobal("sessionStorage", {
    getItem: storage.getItem.bind(storage),
    setItem: storage.setItem.bind(storage),
    removeItem: storage.removeItem.bind(storage),
    clear: storage.clear.bind(storage),
  });
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

test("unavailable owner is not absence and cannot invite a replacement start", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () =>
      Response.json({
        ...absent(),
        current: owner,
        observation: {
          kind: "unavailable",
          diagnostics: [{
            code: "runtime_unavailable",
            severity: "warning",
            message: "Runtime cannot be observed",
          }],
        },
      })
    ),
  );
  render(SubjectBody, props);
  await screen.findByText("Body owner present; observation unavailable.");
  expect(screen.getByText("Runtime cannot be observed")).toBeTruthy();
  expect(
    screen.queryByRole("button", {
      name: /^(Start|Restart|Stop) conversation body$/,
    }),
  ).toBeNull();
  expect(screen.queryByText("No current conversation body."))
    .toBeNull();
  expect(screen.queryByText(owner.worker_id)).toBeNull();
});

test("unauthorized controls are hidden and permission diagnosis stays visible", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () =>
      Response.json({ ...absent(), can_start: false, can_stop: false })
    ),
  );
  render(SubjectBody, props);
  await screen.findByText(/You do not have permission to start or stop/);
  expect(screen.queryByRole("button", { name: "Start conversation body" }))
    .toBeNull();
  expect(screen.queryByRole("button", { name: "Stop conversation body" }))
    .toBeNull();
  expect(screen.getByRole("button", { name: "Refresh body state" }))
    .toBeTruthy();
});

test("unknown survives refresh and remount and retries the complete original request", async () => {
  const posts: string[] = [];
  let observation: unknown = absent();
  vi.stubGlobal(
    "fetch",
    vi.fn(async (_path: string, init?: RequestInit) => {
      if (init?.method !== "POST") return Response.json(observation);
      posts.push(init.body as string);
      const request = JSON.parse(init.body as string);
      return Response.json({
        request_id: request.request_id,
        disposition: "unknown",
        diagnostics: [],
        body: observation,
      });
    }),
  );
  const first = render(SubjectBody, props);
  await fireEvent.click(
    await screen.findByRole("button", { name: "Start conversation body" }),
  );
  await screen.findByText("Request outcome unknown.");
  await waitFor(() =>
    expect(
      (screen.getByRole("button", {
        name: "Retry same start request",
      }) as HTMLButtonElement).disabled,
    ).toBe(false)
  );
  const saved = sessionStorage.getItem(
    subjectBodyStorageKey(props.accountId, props.workspaceId, props.subjectId),
  );
  expect(saved).not.toBeNull();
  expect(JSON.parse(posts[0])).toMatchObject({
    expected_current: null,
    expected_observation_token: null,
  });
  observation = available("stopped", "new-observation");
  await fireEvent.click(
    screen.getByRole("button", { name: "Refresh body state" }),
  );
  await screen.findByText("Body stopped.");
  expect(
    sessionStorage.getItem(
      subjectBodyStorageKey(
        props.accountId,
        props.workspaceId,
        props.subjectId,
      ),
    ),
  ).toBe(saved);
  expect(posts).toHaveLength(1);
  first.unmount();
  render(SubjectBody, props);
  await screen.findByText(/An unresolved start request is retained/);
  await waitFor(() =>
    expect(
      (screen.getByRole("button", {
        name: "Retry same start request",
      }) as HTMLButtonElement).disabled,
    ).toBe(false)
  );
  expect(screen.queryByRole("button", { name: "Restart conversation body" }))
    .toBeNull();
  await fireEvent.click(
    screen.getByRole("button", { name: "Retry same start request" }),
  );
  await waitFor(() => expect(posts).toHaveLength(2));
  expect(posts[1]).toBe(posts[0]);
  expect(
    readSubjectBodyAttempt(
      sessionStorage,
      props.accountId,
      props.workspaceId,
      props.subjectId,
    )
      ?.request.expected_current,
  ).toBeNull();
});

test("transport loss retains the sent input without automatic resubmission", async () => {
  const posts: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (_path: string, init?: RequestInit) => {
      if (init?.method !== "POST") return Response.json(available());
      posts.push(init.body as string);
      throw new TypeError("connection lost");
    }),
  );
  render(SubjectBody, props);
  await fireEvent.click(
    await screen.findByRole("button", { name: "Restart conversation body" }),
  );
  await screen.findByText(/Request outcome unknown\. The request may/);
  await waitFor(() =>
    expect(
      (screen.getByRole("button", {
        name: "Retry same start request",
      }) as HTMLButtonElement).disabled,
    ).toBe(false)
  );
  expect(posts).toHaveLength(1);
  expect(
    JSON.stringify(
      readSubjectBodyAttempt(
        sessionStorage,
        props.accountId,
        props.workspaceId,
        props.subjectId,
      )
        ?.request,
    ),
  ).toBe(posts[0]);
  const request = JSON.parse(posts[0]);
  expect(request.expected_current).toEqual(owner);
  expect(request.expected_observation_token).toBe("observation-1");
});

test("accepted stop receipt does not display a stopped body before observation", async () => {
  let stopped = false;
  const posts: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (_path: string, init?: RequestInit) => {
      if (init?.method !== "POST") {
        return Response.json(available(stopped ? "stopped" : "idle"));
      }
      posts.push(init.body as string);
      return Response.json({
        request_id: JSON.parse(init.body as string).request_id,
        disposition: "accepted",
        diagnostics: [],
        body: null,
      });
    }),
  );
  render(SubjectBody, props);
  await fireEvent.click(
    await screen.findByRole("button", { name: "Stop conversation body" }),
  );
  await screen.findByText("Request accepted.");
  await waitFor(() =>
    expect(
      (screen.getByRole("button", {
        name: "Refresh body state",
      }) as HTMLButtonElement).disabled,
    ).toBe(false)
  );
  expect(screen.queryByText("Body stopped.")).toBeNull();
  expect(screen.getByText("Live execution")).toBeTruthy();
  expect(JSON.parse(posts[0])).toMatchObject({
    expected_current: owner,
    expected_observation_token: "observation-1",
  });
  stopped = true;
  await fireEvent.click(
    screen.getByRole("button", { name: "Refresh body state" }),
  );
  await screen.findByText("Body stopped.");
  expect(
    screen.getByText(
      /Start restores this body; retained conversation and Memory/,
    ),
  ).toBeTruthy();
});

test("structured configuration rejection is visible without inventing running state", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (_path: string, init?: RequestInit) =>
      Response.json(
        init?.method !== "POST" ? absent() : {
          request_id: JSON.parse(init.body as string).request_id,
          disposition: "rejected",
          diagnostics: [{
            code: "permission_missing",
            severity: "error",
            message: "Required permission session.record is missing",
          }],
          body: absent(),
        },
      )
    ),
  );
  render(SubjectBody, props);
  await fireEvent.click(
    await screen.findByRole("button", { name: "Start conversation body" }),
  );
  await screen.findByText("Required permission session.record is missing");
  expect(screen.getByText("Request rejected.")).toBeTruthy();
  expect(screen.getByText("No current conversation body."))
    .toBeTruthy();
});

test("storage write failure sends nothing and exposes a recovery diagnostic", async () => {
  const fetch = vi.fn(async () => Response.json(absent()));
  vi.stubGlobal("fetch", fetch);
  render(SubjectBody, props);
  const start = await screen.findByRole("button", {
    name: "Start conversation body",
  });
  vi.spyOn(sessionStorage, "setItem").mockImplementation(() => {
    throw new Error("storage denied");
  });
  await fireEvent.click(start);
  await screen.findByText(/The exact request could not be retained/);
  expect(fetch).toHaveBeenCalledTimes(1);
});

test("late GET for an old route cannot overwrite the selected Subject", async () => {
  const old = deferred<Response>();
  vi.stubGlobal(
    "fetch",
    vi.fn((path: string) =>
      path.includes("/subjects/subject/body") ? old.promise : Promise.resolve(
        Response.json({
          ...absent("other"),
          can_start: false,
          can_stop: false,
        }),
      )
    ),
  );
  const view = render(SubjectBody, props);
  await waitFor(() =>
    expect(screen.getByText("Loading body state…")).toBeTruthy()
  );
  await view.rerender({ ...props, subjectId: "other" });
  await screen.findByText(/You do not have permission to start or stop/);
  const response = Response.json(available());
  const consumed = afterBodyRead(response);
  old.resolve(response);
  await consumed;
  await tick();
  await waitFor(() =>
    expect(screen.getByText("No current conversation body."))
      .toBeTruthy()
  );
  expect(screen.queryByText("Body stopped.")).toBeNull();
  expect(screen.queryByRole("button", { name: "Restart conversation body" }))
    .toBeNull();
});

test("late old-route mutation receipt cannot update new route or discard old recovery input", async () => {
  const pending = deferred<Response>();
  let requestId = "";
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string, init?: RequestInit) => {
      if (init?.method === "POST") {
        requestId = JSON.parse(init.body as string).request_id;
        return pending.promise;
      }
      return Response.json(
        path.includes("/subjects/other/body")
          ? { ...absent("other"), can_start: false, can_stop: false }
          : absent(),
      );
    }),
  );
  const view = render(SubjectBody, props);
  await fireEvent.click(
    await screen.findByRole("button", { name: "Start conversation body" }),
  );
  await screen.findByText("Sending request; body state is not yet confirmed.");
  const saved = sessionStorage.getItem(
    subjectBodyStorageKey(props.accountId, props.workspaceId, props.subjectId),
  );
  await view.rerender({ ...props, subjectId: "other" });
  await screen.findByText(/You do not have permission to start or stop/);
  // 同じrouteへ戻っても、以前のmount世代の応答を採用しない。
  await view.rerender(props);
  await screen.findByText(/An unresolved start request is retained/);
  const response = Response.json({
    request_id: requestId,
    disposition: "accepted",
    diagnostics: [],
    body: available(),
  });
  const consumed = afterBodyRead(response);
  pending.resolve(response);
  await consumed;
  await tick();
  await waitFor(() =>
    expect(screen.getByText("No current conversation body."))
      .toBeTruthy()
  );
  expect(screen.queryByText("Request accepted.")).toBeNull();
  expect(screen.queryByText("Body stopped.")).toBeNull();
  expect(
    sessionStorage.getItem(
      subjectBodyStorageKey(
        props.accountId,
        props.workspaceId,
        props.subjectId,
      ),
    ),
  ).toBe(saved);
});

test("a newer refresh observation is not replaced by an older mutation body", async () => {
  const mutation = deferred<Response>();
  const finalRefresh = deferred<Response>();
  let gets = 0;
  let requestId = "";
  vi.stubGlobal(
    "fetch",
    vi.fn(async (_path: string, init?: RequestInit) => {
      if (init?.method === "POST") {
        requestId = JSON.parse(init.body as string).request_id;
        return mutation.promise;
      }
      gets += 1;
      if (gets === 1) return Response.json(absent());
      if (gets === 2) return Response.json(available("idle"));
      return finalRefresh.promise;
    }),
  );
  render(SubjectBody, props);
  await fireEvent.click(
    await screen.findByRole("button", { name: "Start conversation body" }),
  );
  await screen.findByText("Sending request; body state is not yet confirmed.");
  await fireEvent.click(
    screen.getByRole("button", { name: "Refresh body state" }),
  );
  await screen.findByText("Live execution");
  mutation.resolve(
    Response.json({
      request_id: requestId,
      disposition: "accepted",
      diagnostics: [],
      body: available(),
    }),
  );
  await screen.findByText("Request accepted.");
  await waitFor(() => expect(gets).toBe(3));
  expect(screen.queryByText("Body stopped.")).toBeNull();
  expect(screen.getByText("Live execution")).toBeTruthy();
  finalRefresh.resolve(Response.json(available("idle")));
  await waitFor(() =>
    expect(
      (screen.getByRole("button", {
        name: "Refresh body state",
      }) as HTMLButtonElement).disabled,
    ).toBe(false)
  );
});

test.each(["not_found", "pending"])(
  "read-only %s stop lookup and body GET retain the complete unknown request",
  async (outcome) => {
    const posts: { path: string; body: string }[] = [];
    let observation = available("idle");
    vi.stubGlobal(
      "fetch",
      vi.fn(async (path: string, init?: RequestInit) => {
        if (init?.method !== "POST") return Response.json(observation);
        posts.push({ path, body: init.body as string });
        const request = JSON.parse(init.body as string);
        if (path.endsWith("/stop/outcome")) {
          return Response.json({
            request_id: request.request_id,
            outcome,
            body: observation,
          });
        }
        // 停止した本体のGETを得ても、その停止依頼のunknownは未解決のまま。
        observation = available("stopped", "new-observation");
        return Response.json({
          request_id: request.request_id,
          disposition: "unknown",
          diagnostics: [],
          body: null,
        });
      }),
    );
    const view = render(SubjectBody, props);
    await fireEvent.click(
      await screen.findByRole("button", { name: "Stop conversation body" }),
    );
    await screen.findByText("Request outcome unknown.");
    await screen.findByText("Body stopped.");
    const key = subjectBodyStorageKey(
      props.accountId,
      props.workspaceId,
      props.subjectId,
    );
    const saved = sessionStorage.getItem(key);
    expect(saved).not.toBeNull();
    await waitFor(() =>
      expect(
        (screen.getByRole("button", {
          name: "Refresh body state",
        }) as HTMLButtonElement).disabled,
      ).toBe(false)
    );
    await fireEvent.click(
      screen.getByRole("button", { name: "Refresh body state" }),
    );
    await waitFor(() =>
      expect(
        (screen.getByRole("button", {
          name: "Refresh body state",
        }) as HTMLButtonElement).disabled,
      ).toBe(false)
    );
    expect(sessionStorage.getItem(key)).toBe(saved);
    expect(screen.queryByRole("button", { name: "Restart conversation body" }))
      .toBeNull();
    // 再読込み相当のremount後も、初回停止時の全入力で照会する。
    view.unmount();
    render(SubjectBody, props);
    await fireEvent.click(
      await screen.findByRole("button", { name: "Check stop outcome" }),
    );
    await screen.findByText(
      outcome === "pending"
        ? "Stop outcome pending."
        : "No stop receipt found.",
    );
    await waitFor(() =>
      expect(
        (screen.getByRole("button", {
          name: "Refresh body state",
        }) as HTMLButtonElement).disabled,
      ).toBe(false)
    );
    expect(posts.filter((request) => request.path.endsWith("/stop")))
      .toHaveLength(1);
    const checks = posts.filter((request) =>
      request.path.endsWith("/stop/outcome")
    );
    expect(checks).toHaveLength(1);
    expect(checks[0].body).toBe(posts[0].body);
    expect(JSON.parse(checks[0].body)).toMatchObject({
      expected_current: owner,
      expected_observation_token: "observation-1",
    });
    expect(sessionStorage.getItem(key)).toBe(saved);
    expect(screen.queryByRole("button", { name: "Restart conversation body" }))
      .toBeNull();
    expect(screen.getByRole("button", { name: "Retry same stop request" }))
      .toBeTruthy();
  },
);

test("historical stopped outcome settles only the old request while restored body remains running", async () => {
  const posts: { path: string; body: string }[] = [];
  let observation = available("idle");
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string, init?: RequestInit) => {
      if (init?.method !== "POST") return Response.json(observation);
      posts.push({ path, body: init.body as string });
      const request = JSON.parse(init.body as string);
      if (path.endsWith("/stop/outcome")) {
        observation = available("running", "restored-observation");
        return Response.json({
          request_id: request.request_id,
          outcome: "stopped",
          body: observation,
        });
      }
      return Response.json({
        request_id: request.request_id,
        disposition: "unknown",
        diagnostics: [],
        body: null,
      });
    }),
  );
  render(SubjectBody, props);
  await fireEvent.click(
    await screen.findByRole("button", { name: "Stop conversation body" }),
  );
  await screen.findByText("Request outcome unknown.");
  await fireEvent.click(
    screen.getByRole("button", { name: "Check stop outcome" }),
  );
  await screen.findByText("Historical stop confirmed.");
  await screen.findByText("running");
  expect(screen.queryByText("Body stopped.")).toBeNull();
  expect(screen.queryByText("Request outcome unknown.")).toBeNull();
  expect(screen.getByText(/The body may have been restored since then/))
    .toBeTruthy();
  expect(
    sessionStorage.getItem(
      subjectBodyStorageKey(
        props.accountId,
        props.workspaceId,
        props.subjectId,
      ),
    ),
  ).toBeNull();
  expect(posts.filter((request) => request.path.endsWith("/stop")))
    .toHaveLength(1);
  expect(posts[1].path.endsWith("/stop/outcome")).toBe(true);
  expect(posts[1].body).toBe(posts[0].body);
});

test.each([
  "forbidden",
  "lookup",
  "http-not-found",
  "transport",
  "wrong-receipt",
  "invalid-json",
])(
  "%s stop lookup failure is not a missing receipt or settled stop",
  async (failure) => {
    const posts: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (path: string, init?: RequestInit) => {
        if (init?.method !== "POST") return Response.json(available("idle"));
        posts.push(path);
        if (path.endsWith("/stop/outcome")) {
          if (failure === "transport") throw new Error("connection lost");
          if (
            failure === "forbidden" || failure === "lookup" ||
            failure === "http-not-found"
          ) {
            return Response.json({ error: "unavailable" }, {
              status: failure === "forbidden"
                ? 403
                : failure === "http-not-found"
                ? 404
                : 502,
            });
          }
          if (failure === "invalid-json") return new Response("invalid-json");
          return Response.json({
            request_id: "other-request",
            outcome: "not_found",
            body: null,
          });
        }
        return Response.json({
          request_id: JSON.parse(init.body as string).request_id,
          disposition: "unknown",
          diagnostics: [],
          body: null,
        });
      }),
    );
    render(SubjectBody, props);
    await fireEvent.click(
      await screen.findByRole("button", { name: "Stop conversation body" }),
    );
    await screen.findByText("Request outcome unknown.");
    const key = subjectBodyStorageKey(
      props.accountId,
      props.workspaceId,
      props.subjectId,
    );
    const saved = sessionStorage.getItem(key);
    await fireEvent.click(
      screen.getByRole("button", { name: "Check stop outcome" }),
    );
    await screen.findByText(/Stop outcome could not be checked/);
    expect(screen.queryByText("No stop receipt found.")).toBeNull();
    expect(screen.queryByText("Stop outcome pending.")).toBeNull();
    expect(screen.queryByText("Historical stop confirmed.")).toBeNull();
    expect(sessionStorage.getItem(key)).toBe(saved);
    expect(posts.filter((path) => path.endsWith("/stop"))).toHaveLength(1);
    expect(posts.filter((path) => path.endsWith("/stop/outcome"))).toHaveLength(
      1,
    );
  },
);

test("late stop outcome cannot settle an old route request after navigation", async () => {
  const pending = deferred<Response>();
  let requestId = "";
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string, init?: RequestInit) => {
      if (path.endsWith("/stop/outcome")) return pending.promise;
      if (init?.method === "POST") {
        requestId = JSON.parse(init.body as string).request_id;
        return Response.json({
          request_id: requestId,
          disposition: "unknown",
          diagnostics: [],
          body: null,
        });
      }
      return Response.json(
        path.includes("/subjects/other/body")
          ? { ...absent("other"), can_start: false, can_stop: false }
          : available("idle"),
      );
    }),
  );
  const view = render(SubjectBody, props);
  await fireEvent.click(
    await screen.findByRole("button", { name: "Stop conversation body" }),
  );
  await screen.findByText("Request outcome unknown.");
  await fireEvent.click(
    screen.getByRole("button", { name: "Check stop outcome" }),
  );
  await screen.findByText(
    "Checking saved stop outcome… No stop request is sent.",
  );
  const key = subjectBodyStorageKey(
    props.accountId,
    props.workspaceId,
    props.subjectId,
  );
  const saved = sessionStorage.getItem(key);
  await view.rerender({ ...props, subjectId: "other" });
  await screen.findByText(/You do not have permission to start or stop/);
  const response = Response.json({
    request_id: requestId,
    outcome: "stopped",
    body: available(),
  });
  const consumed = afterBodyRead(response);
  pending.resolve(response);
  await consumed;
  await tick();
  expect(screen.queryByText("Historical stop confirmed.")).toBeNull();
  expect(screen.queryByRole("button", { name: "Check stop outcome" }))
    .toBeNull();
  expect(sessionStorage.getItem(key)).toBe(saved);
});

test("account switch hides old body and retry input while same-account reload preserves exact request", async () => {
  const accountB = { ...props, accountId: "account-b" };
  const first = savedStop();
  const keyA = subjectBodyStorageKey(
    props.accountId,
    props.workspaceId,
    props.subjectId,
  );
  const keyB = subjectBodyStorageKey(
    accountB.accountId,
    props.workspaceId,
    props.subjectId,
  );
  const rawA = JSON.stringify(first);
  sessionStorage.setItem(keyA, rawA);
  const pendingB = deferred<Response>();
  let account = props.accountId;
  let firstB = true;
  const posts: { path: string; body: string }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string, init?: RequestInit) => {
      if (init?.method === "POST") {
        posts.push({ path, body: init.body as string });
        return Response.json({
          request_id: JSON.parse(init.body as string).request_id,
          disposition: "unknown",
          diagnostics: [],
          body: null,
        });
      }
      if (account === accountB.accountId && firstB) {
        firstB = false;
        return pendingB.promise;
      }
      return Response.json(
        account === props.accountId ? available() : absent(),
      );
    }),
  );
  const view = render(SubjectBody, props);
  await screen.findByText("Body stopped.");
  await screen.findByText(/An unresolved stop request is retained/);
  const reads = vi.spyOn(sessionStorage, "getItem");
  reads.mockClear();
  account = accountB.accountId;
  await view.rerender(accountB);
  await screen.findByText("Loading body state…");
  expect(screen.queryByText("Body stopped.")).toBeNull();
  expect(screen.queryByText(/An unresolved stop request is retained/))
    .toBeNull();
  expect(screen.queryByText(first.request.request_id)).toBeNull();
  expect(screen.queryByRole("button", { name: "Retry same stop request" }))
    .toBeNull();
  expect(reads).toHaveBeenCalledWith(keyB);
  expect(reads).not.toHaveBeenCalledWith(keyA);
  pendingB.resolve(Response.json(absent()));
  await fireEvent.click(
    await screen.findByRole("button", { name: "Start conversation body" }),
  );
  await screen.findByText("Request outcome unknown.");
  const rawB = sessionStorage.getItem(keyB);
  expect(rawB).not.toBeNull();
  expect(JSON.parse(rawB!)).toMatchObject({ accountId: accountB.accountId });
  expect(sessionStorage.getItem(keyA)).toBe(rawA);
  expect(posts).toHaveLength(1);
  expect(JSON.parse(posts[0].body).request_id).not.toBe(
    first.request.request_id,
  );
  expect(JSON.parse(posts[0].body)).not.toHaveProperty("accountId");
  view.unmount();
  const reloadedB = render(SubjectBody, accountB);
  await screen.findByText(/An unresolved start request is retained/);
  expect(screen.queryByText(first.request.request_id)).toBeNull();
  expect(screen.queryByRole("button", { name: "Check stop outcome" }))
    .toBeNull();
  expect(posts).toHaveLength(1);
  reloadedB.unmount();
  account = props.accountId;
  render(SubjectBody, props);
  await waitFor(() =>
    expect(
      (screen.getByRole("button", {
        name: "Retry same stop request",
      }) as HTMLButtonElement).disabled,
    ).toBe(false)
  );
  await fireEvent.click(
    screen.getByRole("button", { name: "Retry same stop request" }),
  );
  await waitFor(() => expect(posts).toHaveLength(2));
  expect(posts[1].path.endsWith("/stop")).toBe(true);
  expect(posts[1].body).toBe(JSON.stringify(first.request));
  expect(JSON.parse(posts[1].body)).not.toHaveProperty("accountId");
  expect(sessionStorage.getItem(keyA)).toBe(rawA);
  expect(sessionStorage.getItem(keyB)).toBe(rawB);
});

test("late GET from another account cannot replace the same route's new observation", async () => {
  const pendingA = deferred<Response>();
  let account = props.accountId;
  const fetch = vi.fn(() =>
    account === props.accountId ? pendingA.promise : Promise.resolve(
      Response.json({ ...absent(), can_start: false, can_stop: false }),
    )
  );
  vi.stubGlobal("fetch", fetch);
  const view = render(SubjectBody, props);
  await waitFor(() => expect(fetch).toHaveBeenCalledTimes(1));
  account = "account-b";
  await view.rerender({ ...props, accountId: account });
  await screen.findByText(/You do not have permission to start or stop/);
  const oldResponse = Response.json(available());
  const consumed = afterBodyRead(oldResponse);
  pendingA.resolve(oldResponse);
  await consumed;
  await tick();
  expect(screen.getByText("No current conversation body.")).toBeTruthy();
  expect(screen.queryByText("Body stopped.")).toBeNull();
  expect(screen.queryByRole("button", { name: "Restart conversation body" }))
    .toBeNull();
  expect(fetch).toHaveBeenCalledTimes(2);
});

test.each([false, true])(
  "late mutation cannot update or clear another account's requests, round trip=%s",
  async (roundTrip) => {
    const pendingA = deferred<Response>();
    const accountB = { ...props, accountId: "account-b" };
    let account = props.accountId;
    let requestA = "";
    let gets = 0;
    const posts: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (_path: string, init?: RequestInit) => {
        if (init?.method !== "POST") {
          gets += 1;
          return Response.json(absent());
        }
        posts.push(init.body as string);
        if (account === props.accountId) {
          requestA = JSON.parse(init.body as string).request_id;
          return pendingA.promise;
        }
        return Response.json({
          request_id: JSON.parse(init.body as string).request_id,
          disposition: "unknown",
          diagnostics: [],
          body: null,
        });
      }),
    );
    const view = render(SubjectBody, props);
    await fireEvent.click(
      await screen.findByRole("button", { name: "Start conversation body" }),
    );
    await screen.findByText(
      "Sending request; body state is not yet confirmed.",
    );
    const keyA = subjectBodyStorageKey(
      props.accountId,
      props.workspaceId,
      props.subjectId,
    );
    const rawA = sessionStorage.getItem(keyA);
    account = accountB.accountId;
    await view.rerender(accountB);
    expect(
      screen.queryByText("Sending request; body state is not yet confirmed."),
    ).toBeNull();
    await fireEvent.click(
      await screen.findByRole("button", { name: "Start conversation body" }),
    );
    await screen.findByText("Request outcome unknown.");
    await waitFor(() =>
      expect(
        (screen.getByRole("button", {
          name: "Refresh body state",
        }) as HTMLButtonElement).disabled,
      ).toBe(false)
    );
    const keyB = subjectBodyStorageKey(
      accountB.accountId,
      props.workspaceId,
      props.subjectId,
    );
    const rawB = sessionStorage.getItem(keyB);
    if (roundTrip) {
      account = props.accountId;
      await view.rerender(props);
      await screen.findByText(/An unresolved start request is retained/);
      expect(screen.queryByText("Request outcome unknown.")).toBeNull();
      await waitFor(() =>
        expect(
          (screen.getByRole("button", {
            name: "Refresh body state",
          }) as HTMLButtonElement).disabled,
        ).toBe(false)
      );
    }
    const removals = vi.spyOn(sessionStorage, "removeItem");
    const getsBeforeResponse = gets;
    const oldResponse = Response.json({
      request_id: requestA,
      disposition: "accepted",
      diagnostics: [],
      body: available(),
    });
    const consumed = afterBodyRead(oldResponse);
    pendingA.resolve(oldResponse);
    await consumed;
    await tick();
    expect(screen.queryByText("Request accepted.")).toBeNull();
    expect(screen.queryByText("Body stopped.")).toBeNull();
    expect(removals).not.toHaveBeenCalled();
    expect(sessionStorage.getItem(keyA)).toBe(rawA);
    expect(sessionStorage.getItem(keyB)).toBe(rawB);
    expect(posts).toHaveLength(2);
    expect(gets).toBe(getsBeforeResponse);
  },
);

test("late stop outcome from another account cannot settle or clear either account's saved request", async () => {
  const first = savedStop();
  const second = savedStop("account-b");
  const keyA = subjectBodyStorageKey(
    first.accountId,
    props.workspaceId,
    props.subjectId,
  );
  const keyB = subjectBodyStorageKey(
    second.accountId,
    props.workspaceId,
    props.subjectId,
  );
  const rawA = JSON.stringify(first);
  const rawB = JSON.stringify(second);
  sessionStorage.setItem(keyA, rawA);
  sessionStorage.setItem(keyB, rawB);
  const pending = deferred<Response>();
  const posts: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string, init?: RequestInit) => {
      if (init?.method === "POST") {
        posts.push(init.body as string);
        return pending.promise;
      }
      return Response.json(available("idle"));
    }),
  );
  const view = render(SubjectBody, props);
  await fireEvent.click(
    await screen.findByRole("button", { name: "Check stop outcome" }),
  );
  await screen.findByText(
    "Checking saved stop outcome… No stop request is sent.",
  );
  await view.rerender({ ...props, accountId: second.accountId });
  await screen.findByText(/An unresolved stop request is retained/);
  expect(
    screen.queryByText("Checking saved stop outcome… No stop request is sent."),
  ).toBeNull();
  const removals = vi.spyOn(sessionStorage, "removeItem");
  const oldResponse = Response.json({
    request_id: first.request.request_id,
    outcome: "stopped",
    body: available(),
  });
  const consumed = afterBodyRead(oldResponse);
  pending.resolve(oldResponse);
  await consumed;
  await tick();
  expect(screen.queryByText("Historical stop confirmed.")).toBeNull();
  expect(screen.queryByText("Body stopped.")).toBeNull();
  expect(screen.getByRole("button", { name: "Check stop outcome" }))
    .toBeTruthy();
  expect(removals).not.toHaveBeenCalled();
  expect(posts).toEqual([JSON.stringify(first.request)]);
  expect(sessionStorage.getItem(keyA)).toBe(rawA);
  expect(sessionStorage.getItem(keyB)).toBe(rawB);
});

test("signout hides body and request details and never falls back to a dummy identity", async () => {
  const first = savedStop();
  const key = subjectBodyStorageKey(
    first.accountId,
    props.workspaceId,
    props.subjectId,
  );
  const raw = JSON.stringify(first);
  sessionStorage.setItem(key, raw);
  const fetch = vi.fn(async () => Response.json(available()));
  vi.stubGlobal("fetch", fetch);
  const view = render(SubjectBody, props);
  await screen.findByText("Body stopped.");
  expect(screen.getByText(first.request.request_id)).toBeTruthy();
  const reads = vi.spyOn(sessionStorage, "getItem");
  reads.mockClear();
  await view.rerender({ ...props, accountId: "" });
  await screen.findByText("Sign in to view this conversation body.");
  expect(screen.queryByText("Body stopped.")).toBeNull();
  expect(screen.queryByText(first.request.request_id)).toBeNull();
  expect(screen.queryByRole("button", { name: "Check stop outcome" }))
    .toBeNull();
  expect(screen.queryByRole("button", { name: "Refresh body state" }))
    .toBeNull();
  expect(reads).not.toHaveBeenCalled();
  expect(fetch).toHaveBeenCalledTimes(1);
  expect(sessionStorage.getItem(key)).toBe(raw);
});

test("Workspace switch fences late mutation even when the Subject ID is identical", async () => {
  const pending = deferred<Response>();
  let requestId = "";
  const other = { ...props, workspaceId: "other-workspace" };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string, init?: RequestInit) => {
      if (init?.method === "POST") {
        requestId = JSON.parse(init.body as string).request_id;
        return pending.promise;
      }
      return Response.json(
        path.includes("/other-workspace/")
          ? {
            ...absent(),
            workspace_id: other.workspaceId,
            can_start: false,
            can_stop: false,
          }
          : absent(),
      );
    }),
  );
  const view = render(SubjectBody, props);
  await fireEvent.click(
    await screen.findByRole("button", { name: "Start conversation body" }),
  );
  const key = subjectBodyStorageKey(
    props.accountId,
    props.workspaceId,
    props.subjectId,
  );
  const retained = sessionStorage.getItem(key);
  await view.rerender(other);
  await screen.findByText(/You do not have permission to start or stop/);
  const response = Response.json({
    request_id: requestId,
    disposition: "accepted",
    diagnostics: [],
    body: available(),
  });
  const consumed = afterBodyRead(response);
  pending.resolve(response);
  await consumed;
  await tick();
  expect(screen.queryByText("Request accepted.")).toBeNull();
  expect(screen.queryByText("Body stopped.")).toBeNull();
  expect(sessionStorage.getItem(key)).toBe(retained);
  expect(
    readSubjectBodyAttempt(
      sessionStorage,
      props.accountId,
      other.workspaceId,
      props.subjectId,
    ),
  ).toBeNull();
  await view.rerender(props);
  await screen.findByText(/An unresolved start request is retained/);
});

test("double click sends only one retained request, never a conversation ACK", async () => {
  const pending = deferred<Response>();
  const posts: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (_path: string, init?: RequestInit) => {
      if (init?.method !== "POST") return Response.json(absent());
      posts.push(init.body as string);
      return pending.promise;
    }),
  );
  render(SubjectBody, props);
  const start = await screen.findByRole("button", {
    name: "Start conversation body",
  });
  await fireEvent.click(start);
  await fireEvent.click(start);
  expect(posts).toHaveLength(1);
  // A chat-submit acknowledgement is not a body mutation receipt.
  const response = Response.json({
    command_id: "chat-command",
    state: "accepted",
  });
  const consumed = afterBodyRead(response);
  pending.resolve(response);
  await consumed;
  await screen.findByText(/Request outcome unknown\. The request may/);
  expect(
    readSubjectBodyAttempt(
      sessionStorage,
      props.accountId,
      props.workspaceId,
      props.subjectId,
    )?.request,
  )
    .toEqual(JSON.parse(posts[0]));
  expect(screen.queryByText("Request accepted.")).toBeNull();
});

test("old generation rejection stays distinct from fresh body and requires a fresh explicit stop", async () => {
  let observation = available("idle");
  const posts: {
    expected_current: typeof owner;
    expected_observation_token: string;
  }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (_path: string, init?: RequestInit) => {
      if (init?.method !== "POST") return Response.json(observation);
      const request = JSON.parse(init.body as string);
      posts.push(request);
      observation = available("idle", "generation-18-token");
      observation.current = { ...owner, generation: "18" };
      return Response.json({
        request_id: request.request_id,
        disposition: "rejected",
        diagnostics: [{
          code: "stale_owner",
          severity: "warning",
          message:
            "The current body generation changed. Refresh before acting.",
        }],
        body: observation,
      });
    }),
  );
  render(SubjectBody, props);
  await fireEvent.click(
    await screen.findByRole("button", { name: "Stop conversation body" }),
  );
  await screen.findByText("Request rejected.");
  await screen.findByText(
    "The current body generation changed. Refresh before acting.",
  );
  await waitFor(() =>
    expect(
      (screen.getByRole("button", {
        name: "Stop conversation body",
      }) as HTMLButtonElement).disabled,
    ).toBe(false)
  );
  expect(posts).toHaveLength(1);
  expect(posts[0].expected_current.generation).toBe("17");
  expect(screen.queryByText("Body stopped.")).toBeNull();
  await fireEvent.click(
    screen.getByRole("button", { name: "Stop conversation body" }),
  );
  await waitFor(() => expect(posts).toHaveLength(2));
  expect(posts[1].expected_current.generation).toBe("18");
  expect(posts[1].expected_observation_token).toBe("generation-18-token");
});

test("unknown stop cannot restart on GET alone; confirmed outcome permits restart with fresh token", async () => {
  let observation = available("idle");
  let outcome = "pending";
  const posts: { path: string; body: string }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string, init?: RequestInit) => {
      if (init?.method !== "POST") return Response.json(observation);
      posts.push({ path, body: init.body as string });
      const request = JSON.parse(init.body as string);
      if (path.endsWith("/outcome")) {
        return Response.json({
          request_id: request.request_id,
          outcome,
          body: null,
        });
      }
      return Response.json({
        request_id: request.request_id,
        disposition: "unknown",
        diagnostics: [],
        body: null,
      });
    }),
  );
  render(SubjectBody, props);
  await fireEvent.click(
    await screen.findByRole("button", { name: "Stop conversation body" }),
  );
  await screen.findByText("Request outcome unknown.");
  await waitFor(() =>
    expect(
      (screen.getByRole("button", {
        name: "Refresh body state",
      }) as HTMLButtonElement).disabled,
    ).toBe(false)
  );
  observation = available("stopped", "after-confirmed-stop");
  await fireEvent.click(
    screen.getByRole("button", { name: "Refresh body state" }),
  );
  await screen.findByText("Body stopped.");
  expect(screen.queryByRole("button", { name: "Restart conversation body" }))
    .toBeNull();
  await fireEvent.click(
    screen.getByRole("button", { name: "Check stop outcome" }),
  );
  await screen.findByText("Stop outcome pending.");
  expect(screen.queryByRole("button", { name: "Restart conversation body" }))
    .toBeNull();
  outcome = "stopped";
  await fireEvent.click(
    screen.getByRole("button", { name: "Check stop outcome" }),
  );
  await screen.findByText("Historical stop confirmed.");
  const restart = await screen.findByRole("button", {
    name: "Restart conversation body",
  });
  await waitFor(() =>
    expect((restart as HTMLButtonElement).disabled).toBe(false)
  );
  await fireEvent.click(restart);
  await waitFor(() => expect(posts).toHaveLength(4));
  expect(posts[1].body).toBe(posts[0].body);
  expect(posts[2].body).toBe(posts[0].body);
  const request = JSON.parse(posts[3].body);
  expect(posts[3].path).toMatch(/\/start$/);
  expect(request.expected_current).toEqual(owner);
  expect(request.expected_observation_token).toBe("after-confirmed-stop");
  expect(request.request_id).not.toBe(JSON.parse(posts[0].body).request_id);
});
