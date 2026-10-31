import {
  clearSubjectBodyAttempt,
  createSubjectBodyAttempt,
  getSubjectBody,
  getSubjectBodyStopOutcome,
  parseSubjectBodyMutationResponse,
  parseSubjectBodyResponse,
  parseSubjectBodyStopOutcomeResponse,
  readSubjectBodyAttempt,
  saveSubjectBodyAttempt,
  startSubjectBody,
  stopSubjectBody,
  SubjectBodyApiError,
  subjectBodyControls,
  subjectBodyStorageKey,
} from "./subject-body.ts";

declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};
function assert(value: unknown): asserts value {
  if (!value) throw new Error("Assertion failed");
}
function rejects(fn: () => unknown): void {
  let rejected = false;
  try {
    fn();
  } catch {
    rejected = true;
  }
  assert(rejected);
}
const owner = {
  runtime_id: "runtime",
  worker_id: "worker",
  generation: "18446744073709551615",
};
function absent() {
  return {
    workspace_id: "workspace",
    subject_id: "subject",
    current: null,
    observation: { kind: "absent" },
    can_start: true,
    can_stop: true,
  };
}
function available(state = "stopped") {
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
        restore_observation_token: "observation-1",
      },
    },
  };
}
function parse(value: unknown) {
  return parseSubjectBodyResponse(value, "workspace", "subject");
}
function storage() {
  const values = new Map<string, string>();
  return {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => {
      values.set(key, value);
    },
    removeItem: (key: string) => {
      values.delete(key);
    },
  };
}

Deno.test("absent permits initial start while unavailable retains owner and forbids replacement", () => {
  assert(subjectBodyControls(parse(absent())).start);
  const unavailable = parse({
    ...absent(),
    current: owner,
    observation: {
      kind: "unavailable",
      diagnostics: [{
        code: "runtime_unavailable",
        severity: "warning",
        message: "Cannot observe Runtime",
      }],
    },
  });
  assert(unavailable.current?.generation === "18446744073709551615");
  assert(
    !subjectBodyControls(unavailable).start &&
      !subjectBodyControls(unavailable).stop,
  );
  rejects(() => parse({ ...absent(), current: owner }));
  rejects(() =>
    parse({
      ...absent(),
      observation: { kind: "unavailable", diagnostics: [] },
    })
  );
});

Deno.test("ordinary Worker parsing keeps availability and live execution distinct from lifecycle", () => {
  const stopped = parse(available());
  assert(
    subjectBodyControls(stopped).start && !subjectBodyControls(stopped).stop,
  );
  const idle = parse(available("idle"));
  assert(subjectBodyControls(idle).stop && !subjectBodyControls(idle).start);
  const unobserved = available("idle");
  unobserved.observation.worker.availability = "unavailable";
  unobserved.observation.worker.worker_state = null;
  assert(
    !subjectBodyControls(parse(unobserved)).start &&
      !subjectBodyControls(parse(unobserved)).stop,
  );
  const invalid = available();
  invalid.observation.worker.availability = "invented";
  rejects(() => parse(invalid));
  const noPermission = parse({
    ...available("idle"),
    can_start: false,
    can_stop: false,
  });
  assert(
    !subjectBodyControls(noPermission).start &&
      !subjectBodyControls(noPermission).stop,
  );
});

Deno.test("body responses reject cross-route identities and mismatched owner Worker", () => {
  rejects(() => parse({ ...absent(), workspace_id: "other" }));
  rejects(() => parse({ ...absent(), subject_id: "other" }));
  const mismatch = available();
  mismatch.observation.worker.worker_id = "old-worker";
  rejects(() => parse(mismatch));
  const foreignWorkspace = available();
  foreignWorkspace.observation.worker.workspace.workspace_id = "other";
  rejects(() => parse(foreignWorkspace));
});

Deno.test("external body JSON requires nullable current and bounds generations diagnostics and unknown fields", () => {
  const missing: Record<string, unknown> = absent();
  delete missing.current;
  rejects(() => parse(missing));
  for (
    const generation of [1, "1e3", "-1", "18446744073709551616", "1".repeat(30)]
  ) {
    rejects(() => parse({ ...available(), current: { ...owner, generation } }));
  }
  rejects(() => parse({ ...absent(), can_start: "true" }));
  rejects(() => parse({ ...absent(), extra: true }));
  rejects(() =>
    parse({
      ...available(),
      observation: {
        kind: "unavailable",
        diagnostics: Array(129).fill({
          code: "x",
          severity: "warning",
          message: "x",
        }),
      },
    })
  );
});

Deno.test("mutation receipts validate request identity and nested route without deriving execution state", () => {
  const receipt = {
    request_id: "request",
    disposition: "accepted",
    diagnostics: [],
    body: absent(),
  };
  const parsed = parseSubjectBodyMutationResponse(
    receipt,
    "workspace",
    "subject",
    "request",
  );
  assert(
    parsed.disposition === "accepted" &&
      parsed.body?.observation.kind === "absent",
  );
  rejects(() =>
    parseSubjectBodyMutationResponse(
      receipt,
      "workspace",
      "subject",
      "different",
    )
  );
  rejects(() =>
    parseSubjectBodyMutationResponse(receipt, "other", "subject", "request")
  );
  rejects(() =>
    parseSubjectBodyMutationResponse(
      { ...receipt, disposition: "running" },
      "workspace",
      "subject",
      "request",
    )
  );
});

Deno.test("durable retry input is frozen complete account-and-route-scoped and never silently replaced", () => {
  const saved = storage();
  const attempt = createSubjectBodyAttempt(
    "account",
    parse(available()),
    "start",
    "stable-request",
  );
  saveSubjectBodyAttempt(saved, "account", attempt);
  const restored = readSubjectBodyAttempt(
    saved,
    "account",
    "workspace",
    "subject",
  );
  assert(restored && JSON.stringify(restored) === JSON.stringify(attempt));
  assert(
    Object.isFrozen(restored) && Object.isFrozen(restored.request) &&
      Object.isFrozen(restored.request.expected_current),
  );
  assert(restored.request.expected_observation_token === "observation-1");
  assert(
    readSubjectBodyAttempt(saved, "account", "workspace", "other") === null,
  );
  const newer = createSubjectBodyAttempt(
    "account",
    parse(available()),
    "start",
    "new-request",
  );
  rejects(() => saveSubjectBodyAttempt(saved, "account", newer));
  rejects(() => clearSubjectBodyAttempt(saved, "account", newer));
  saveSubjectBodyAttempt(saved, "account", restored);
  clearSubjectBodyAttempt(saved, "account", restored);
  assert(
    readSubjectBodyAttempt(saved, "account", "workspace", "subject") === null,
  );
});

Deno.test("unreadable or incomplete saved request blocks replacement rather than generating identity", () => {
  const saved = storage();
  const key = subjectBodyStorageKey("account", "workspace", "subject");
  for (
    const raw of [
      "not-json",
      "x".repeat(17000),
      JSON.stringify({
        accountId: "account",
        workspaceId: "workspace",
        subjectId: "subject",
        action: "start",
        request: { request_id: "only-id" },
      }),
      JSON.stringify({
        accountId: "account",
        workspaceId: "workspace",
        subjectId: "other",
        action: "start",
        request: {
          request_id: "request",
          expected_current: null,
          expected_observation_token: null,
        },
      }),
    ]
  ) {
    saved.setItem(key, raw);
    rejects(() =>
      readSubjectBodyAttempt(saved, "account", "workspace", "subject")
    );
    rejects(() =>
      saveSubjectBodyAttempt(
        saved,
        "account",
        createSubjectBodyAttempt(
          "account",
          parse(absent()),
          "start",
          "replacement",
        ),
      )
    );
    assert(saved.getItem(key) === raw);
  }
});

Deno.test("shared HTTP functions encode route segments and preserve exact start stop inputs", async () => {
  const workspace = "space /";
  const subject = "subject /";
  const requests: { path: string; init?: RequestInit }[] = [];
  const body = { ...absent(), workspace_id: workspace, subject_id: subject };
  const fetchFn = ((path: string, init?: RequestInit) => {
    requests.push({ path, init });
    return Promise.resolve(Response.json(
      init?.method === "POST"
        ? {
          request_id: JSON.parse(init.body as string).request_id,
          disposition: "unknown",
          diagnostics: [],
          body,
        }
        : body,
    ));
  }) as typeof fetch;
  const start = {
    request_id: "same-start",
    expected_current: null,
    expected_observation_token: null,
  };
  const stop = {
    request_id: "same-stop",
    expected_current: owner,
    expected_observation_token: "frozen-token",
  };
  await getSubjectBody(fetchFn, workspace, subject);
  const result = await startSubjectBody(fetchFn, workspace, subject, start);
  assert(result.disposition === "unknown");
  await startSubjectBody(fetchFn, workspace, subject, start);
  await stopSubjectBody(fetchFn, workspace, subject, stop);
  assert(
    requests[0].path ===
      "/api/w/space%20%2F/subjektiv/subjects/subject%20%2F/body",
  );
  assert(requests[1].path === `${requests[0].path}/start`);
  assert(requests[3].path === `${requests[0].path}/stop`);
  assert(requests[1].init?.body === JSON.stringify(start));
  assert(requests[2].init?.body === requests[1].init?.body);
  assert(requests[3].init?.body === JSON.stringify(stop));
});

Deno.test("stop outcome validates historical receipt identity independently of current body", () => {
  for (const outcome of ["not_found", "pending", "stopped"]) {
    const result = parseSubjectBodyStopOutcomeResponse(
      {
        request_id: "saved-stop",
        outcome,
        body: available("running"),
      },
      "workspace",
      "subject",
      "saved-stop",
    );
    assert(result.outcome === outcome);
    assert(
      result.body?.observation.kind === "available" &&
        result.body.observation.worker.state === "running",
    );
  }
  const receipt = {
    request_id: "saved-stop",
    outcome: "stopped",
    body: absent(),
  };
  rejects(() =>
    parseSubjectBodyStopOutcomeResponse(
      receipt,
      "workspace",
      "subject",
      "other-request",
    )
  );
  rejects(() =>
    parseSubjectBodyStopOutcomeResponse(
      receipt,
      "other-workspace",
      "subject",
      "saved-stop",
    )
  );
  rejects(() =>
    parseSubjectBodyStopOutcomeResponse(
      receipt,
      "workspace",
      "other-subject",
      "saved-stop",
    )
  );
  rejects(() =>
    parseSubjectBodyStopOutcomeResponse(
      { ...receipt, outcome: "unknown" },
      "workspace",
      "subject",
      "saved-stop",
    )
  );
  rejects(() =>
    parseSubjectBodyStopOutcomeResponse(
      { ...receipt, body: undefined },
      "workspace",
      "subject",
      "saved-stop",
    )
  );
  rejects(() =>
    parseSubjectBodyStopOutcomeResponse(
      { ...receipt, unexpected: true },
      "workspace",
      "subject",
      "saved-stop",
    )
  );
});

Deno.test("stop outcome query sends only the read-only endpoint with full frozen input unchanged", async () => {
  const saved = storage();
  const attempt = createSubjectBodyAttempt(
    "account",
    parse(available("idle")),
    "stop",
    "saved-stop",
  );
  assert(attempt.action === "stop");
  saveSubjectBodyAttempt(saved, "account", attempt);
  const requests: { path: string; init?: RequestInit }[] = [];
  const fetchFn = ((path: string, init?: RequestInit) => {
    requests.push({ path, init });
    return Promise.resolve(
      Response.json({
        request_id: "saved-stop",
        outcome: "pending",
        body: null,
      }),
    );
  }) as typeof fetch;
  const result = await getSubjectBodyStopOutcome(
    fetchFn,
    "workspace",
    "subject",
    attempt.request,
  );
  assert(result.outcome === "pending");
  assert(requests.length === 1);
  assert(
    requests[0].path ===
      "/api/w/workspace/subjektiv/subjects/subject/body/stop/outcome",
  );
  assert(requests[0].init?.method === "POST");
  assert(requests[0].init?.body === JSON.stringify(attempt.request));
  assert(
    JSON.stringify(
      readSubjectBodyAttempt(saved, "account", "workspace", "subject"),
    ) === JSON.stringify(attempt),
  );
});

Deno.test("stop outcome authorization transport lookup and malformed receipt failures never become not_found", async () => {
  const request = {
    request_id: "saved-stop",
    expected_current: owner,
    expected_observation_token: "saved-token",
  };
  for (
    const failure of [
      "forbidden",
      "lookup",
      "http-not-found",
      "transport",
      "wrong-receipt",
      "invalid-json",
    ]
  ) {
    const fetchFn = (async () => {
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
    }) as typeof fetch;
    let rejected = false;
    try {
      await getSubjectBodyStopOutcome(fetchFn, "workspace", "subject", request);
    } catch {
      rejected = true;
    }
    assert(rejected);
  }
});

Deno.test("request IDs enforce 128 UTF-8 bytes and control-free input before sending or retaining", async () => {
  for (const requestId of ["x".repeat(128), "é".repeat(64)]) {
    const attempt = createSubjectBodyAttempt(
      "account",
      parse(absent()),
      "start",
      requestId,
    );
    assert(attempt.request.request_id === requestId);
  }
  for (
    const requestId of [
      "",
      "x".repeat(129),
      "é".repeat(65),
      "request\n",
      "request\u007f",
      "request\u0085",
    ]
  ) {
    rejects(() =>
      createSubjectBodyAttempt("account", parse(absent()), "start", requestId)
    );
    const saved = storage();
    saved.setItem(
      subjectBodyStorageKey("account", "workspace", "subject"),
      JSON.stringify({
        accountId: "account",
        workspaceId: "workspace",
        subjectId: "subject",
        action: "start",
        request: {
          request_id: requestId,
          expected_current: null,
          expected_observation_token: null,
        },
      }),
    );
    rejects(() =>
      readSubjectBodyAttempt(saved, "account", "workspace", "subject")
    );
    let calls = 0;
    const fetchFn = (() => {
      calls += 1;
      return Promise.resolve(Response.json({}));
    }) as typeof fetch;
    const stop = {
      request_id: requestId,
      expected_current: owner,
      expected_observation_token: "saved-token",
    };
    const start = {
      request_id: requestId,
      expected_current: null,
      expected_observation_token: null,
    };
    for (
      const invoke of [
        () => startSubjectBody(fetchFn, "workspace", "subject", start),
        () => stopSubjectBody(fetchFn, "workspace", "subject", stop),
        () => getSubjectBodyStopOutcome(fetchFn, "workspace", "subject", stop),
      ]
    ) {
      let rejected = false;
      try {
        await invoke();
      } catch {
        rejected = true;
      }
      assert(rejected);
    }
    assert(calls === 0);
  }
});

Deno.test("durable requests are account-scoped and another account cannot read save or clear them", () => {
  const saved = storage();
  const first = createSubjectBodyAttempt(
    "account-a",
    parse(available()),
    "start",
    "first-request",
  );
  saveSubjectBodyAttempt(saved, "account-a", first);
  const firstKey = subjectBodyStorageKey("account-a", "workspace", "subject");
  const secondKey = subjectBodyStorageKey("account-b", "workspace", "subject");
  assert(
    firstKey ===
      `yoi.subject-body:${
        JSON.stringify(["account-a", "workspace", "subject"])
      }`,
  );
  assert(firstKey !== secondKey);
  const firstRaw = saved.getItem(firstKey);
  const accesses: string[] = [];
  const observed = {
    getItem(key: string) {
      accesses.push(`get:${key}`);
      return saved.getItem(key);
    },
    setItem(key: string, value: string) {
      accesses.push(`set:${key}`);
      saved.setItem(key, value);
    },
    removeItem(key: string) {
      accesses.push(`remove:${key}`);
      saved.removeItem(key);
    },
  };
  assert(
    readSubjectBodyAttempt(observed, "account-b", "workspace", "subject") ===
      null,
  );
  assert(accesses.length === 1 && accesses[0] === `get:${secondKey}`);
  accesses.length = 0;
  rejects(() => saveSubjectBodyAttempt(observed, "account-b", first));
  rejects(() => clearSubjectBodyAttempt(observed, "account-b", first));
  assert(accesses.length === 0);
  const second = createSubjectBodyAttempt(
    "account-b",
    parse(absent()),
    "start",
    "second-request",
  );
  saveSubjectBodyAttempt(saved, "account-b", second);
  clearSubjectBodyAttempt(saved, "account-b", second);
  assert(saved.getItem(firstKey) === firstRaw);
  const restored = readSubjectBodyAttempt(
    saved,
    "account-a",
    "workspace",
    "subject",
  );
  assert(restored?.accountId === "account-a");
  assert(JSON.stringify(restored) === JSON.stringify(first));
  assert(
    Object.isFrozen(restored) && Object.isFrozen(restored.request) &&
      Object.isFrozen(restored.request.expected_current),
  );
});

Deno.test("a foreign or accountless attempt copied into the current account key is rejected unchanged", () => {
  const saved = storage();
  const first = createSubjectBodyAttempt(
    "account-a",
    parse(available("idle")),
    "stop",
    "first-stop",
  );
  const secondKey = subjectBodyStorageKey("account-b", "workspace", "subject");
  const accountless = {
    workspaceId: first.workspaceId,
    subjectId: first.subjectId,
    action: first.action,
    request: first.request,
  };
  for (const raw of [JSON.stringify(first), JSON.stringify(accountless)]) {
    saved.setItem(secondKey, raw);
    rejects(() =>
      readSubjectBodyAttempt(saved, "account-b", "workspace", "subject")
    );
    const replacement = createSubjectBodyAttempt(
      "account-b",
      parse(absent()),
      "start",
      "new-request",
    );
    rejects(() => saveSubjectBodyAttempt(saved, "account-b", replacement));
    assert(saved.getItem(secondKey) === raw);
  }
});

Deno.test("accountless legacy keys are neither discovered migrated nor cleared for a signed-in account", () => {
  const saved = storage();
  const legacyKey = `yoi.subject-body:${
    JSON.stringify(["workspace", "subject"])
  }`;
  saved.setItem(legacyKey, "unattributed-request");
  const accessed: string[] = [];
  const observed = {
    getItem(key: string) {
      accessed.push(key);
      return saved.getItem(key);
    },
  };
  assert(
    readSubjectBodyAttempt(observed, "account-a", "workspace", "subject") ===
      null,
  );
  assert(
    accessed.length === 1 &&
      accessed[0] ===
        subjectBodyStorageKey("account-a", "workspace", "subject"),
  );
  const own = createSubjectBodyAttempt(
    "account-a",
    parse(absent()),
    "start",
    "own-request",
  );
  saveSubjectBodyAttempt(saved, "account-a", own);
  clearSubjectBodyAttempt(saved, "account-a", own);
  assert(saved.getItem(legacyKey) === "unattributed-request");
  rejects(() => subjectBodyStorageKey("", "workspace", "subject"));
  rejects(() =>
    createSubjectBodyAttempt("", parse(absent()), "start", "no-account")
  );
});

Deno.test("mutation failures expose only identity-bound dedicated 409 request resolutions", async () => {
  const identity = {
    workspace_id: "workspace",
    subject_id: "subject",
    request_id: "frozen-request",
  };
  const errorBody = (request: unknown) => ({
    error: "Conflict",
    message: "The saved observation is stale",
    diagnostics: [{
      code: "stale_observation",
      severity: "warning",
      message: "Refresh before a new explicit action",
    }],
    request,
  });
  const input = Object.freeze({
    request_id: identity.request_id,
    expected_current: owner,
    expected_observation_token: "frozen-token",
  });
  const cases: { status: number; body: unknown; resolution: string | null }[] =
    [
      ...["not_accepted", "superseded", "unresolved"].map((resolution) => ({
        status: 409,
        body: errorBody({ ...identity, resolution }),
        resolution,
      })),
      ...["workspace_id", "subject_id", "request_id"].map((field) => ({
        status: 409,
        body: errorBody({
          ...identity,
          [field]: "foreign",
          resolution: "superseded",
        }),
        resolution: null,
      })),
      ...[401, 403, 500, 502].map((status) => ({
        status,
        body: errorBody({ ...identity, resolution: "not_accepted" }),
        resolution: null,
      })),
      { status: 409, body: errorBody(null), resolution: null },
      {
        status: 409,
        body: { error: "Conflict", message: "busy", diagnostics: [] },
        resolution: null,
      },
      {
        status: 409,
        body: errorBody({ ...identity, resolution: "rejected" }),
        resolution: null,
      },
      {
        status: 409,
        body: errorBody({ ...identity, resolution: "superseded", extra: true }),
        resolution: null,
      },
      {
        status: 409,
        body: {
          ...errorBody({ ...identity, resolution: "not_accepted" }),
          diagnostics: "invalid",
        },
        resolution: null,
      },
    ];
  for (const item of cases) {
    for (
      const invoke of [
        startSubjectBody,
        stopSubjectBody,
        getSubjectBodyStopOutcome,
      ]
    ) {
      let sent = "";
      const fetchFn = ((_path: string, init?: RequestInit) => {
        sent = init?.body as string;
        return Promise.resolve(
          Response.json(item.body, { status: item.status }),
        );
      }) as typeof fetch;
      let failure: unknown;
      try {
        await invoke(fetchFn, "workspace", "subject", input);
      } catch (cause) {
        failure = cause;
      }
      assert(failure instanceof SubjectBodyApiError);
      assert((failure.request?.resolution ?? null) === item.resolution);
      assert(sent === JSON.stringify(input));
    }
  }
});

Deno.test("GET cannot supply a mutation resolution even when its error claims the saved request", async () => {
  const fetchFn = (() =>
    Promise.resolve(Response.json({
      error: "Conflict",
      message: "stale",
      diagnostics: [],
      request: {
        workspace_id: "workspace",
        subject_id: "subject",
        request_id: "saved",
        resolution: "superseded",
      },
    }, { status: 409 }))) as typeof fetch;
  let failure: unknown;
  try {
    await getSubjectBody(fetchFn, "workspace", "subject");
  } catch (cause) {
    failure = cause;
  }
  assert(failure instanceof SubjectBodyApiError);
  assert(failure.request === null);
});
