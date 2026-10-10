import type {
  DriveMutationResponse,
  DriveReadTextResponse,
  DriveRequestStatusResponse,
} from "#lib/generated/drive-api.ts";
import { type DriveClient, DriveRequestError } from "./api.ts";
import { DriveController } from "./controller.svelte.ts";
import {
  assert,
  deferred,
  entry,
  equal,
  ref,
  rejects,
  throws,
} from "./test-fixtures.ts";

declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};
function client(patch: Partial<DriveClient> = {}): DriveClient {
  return {
    root: (ws) => Promise.resolve(entry("1", ws, "1", "folder")),
    metadata: (ws, target) =>
      Promise.resolve(
        entry(
          target.node_id,
          ws,
          "9007199254740993",
          target.node_id === "1" ? "folder" : "file",
        ),
      ),
    list: () => Promise.resolve({ entries: [], next_after: null }),
    search: () => Promise.resolve({ entries: [], next_after: null }),
    readText: (ws, target) =>
      Promise.resolve({
        entry: entry(target.node_id, ws),
        text: "abc",
        truncated: false,
      }),
    image: () => Promise.resolve(new Blob(["abc"])),
    mutate: (ws, id) =>
      Promise.resolve({
        request_id: id,
        entry: entry("2", ws, "9007199254740994"),
      }),
    upload: (ws, id) =>
      Promise.resolve({
        request_id: id,
        entry: entry("2", ws, "9007199254740994"),
      }),
    status: (_, id) =>
      Promise.resolve({ request_id: id, state: "uncommitted", response: null }),
    ...patch,
  };
}
function controller(patch: Partial<DriveClient> = {}) {
  let n = 0;
  return new DriveController(client(patch), () => `req-${++n}`);
}
Deno.test("Drive delayed read cannot replace workspace with the same numeric node id", async () => {
  const old = deferred<DriveReadTextResponse>();
  const started = deferred<void>();
  const c = controller({
    readText: (ws, target) => {
      if (ws === "alpha") {
        started.resolve();
        return old.promise;
      }
      return Promise.resolve({
        entry: entry(target.node_id, ws),
        text: "beta",
        truncated: false,
      });
    },
  });
  const first = c.select("alpha", ref());
  await started.promise;
  await c.select("beta", ref("2", "beta"));
  old.resolve({ entry: entry(), text: "old", truncated: false });
  await first;
  equal(c.state.workspaceId, "beta");
  equal(c.state.text, "beta");
  equal(c.state.draft?.text, "beta");
});
Deno.test("Drive A to B to A selection rejects the first A response and late read errors", async () => {
  const old = deferred<DriveReadTextResponse>();
  const started = deferred<void>();
  let reads = 0;
  const c = controller({
    readText: (ws, target) => {
      if (++reads === 1) {
        started.resolve();
        return old.promise;
      }
      return Promise.resolve({
        entry: entry(target.node_id, ws),
        text: "fresh",
        truncated: false,
      });
    },
  });
  const first = c.select("alpha", ref());
  await started.promise;
  await c.select("alpha", ref("3"));
  await c.select("alpha", ref());
  old.reject(new Error("late"));
  await first;
  equal(c.state.entry?.entry.node_id, "2");
  equal(c.state.text, "fresh");
  equal(c.state.error, null);
});
Deno.test("Drive drafts survive workspace and node switches without last_mutation_id rebasing", async () => {
  let last_mutation_id = "9007199254740993";
  const c = controller({
    readText: (ws, target) =>
      Promise.resolve({
        entry: entry(target.node_id, ws, last_mutation_id),
        text: "server",
        truncated: false,
      }),
  });
  await c.select("alpha", ref());
  c.edit("local alpha");
  await c.select("beta", ref("2", "beta"));
  c.edit("local beta");
  last_mutation_id = "9007199254740999";
  await c.select("alpha", ref("3"));
  await c.select("alpha", ref());
  equal(c.state.draft?.text, "local alpha");
  equal(c.state.draft?.expectedMutationId, "9007199254740993");
  equal(c.state.entry?.last_mutation_id, "9007199254740999");
  await c.select("beta", ref("2", "beta"));
  equal(c.state.draft?.text, "local beta");
});
Deno.test("Drive conflict keeps local text and old CAS last_mutation_id across switching without retry", async () => {
  let mutations = 0;
  const c = controller({
    mutate: (_, __, mutation) => {
      ++mutations;
      assert("expected_mutation_id" in mutation);
      equal(mutation.expected_mutation_id, "9007199254740993");
      return Promise.reject(new DriveRequestError("conflict", "not_committed"));
    },
  });
  await c.select("alpha", ref());
  c.edit("local");
  await c.save();
  equal(c.state.draft?.conflict, true);
  equal(c.state.draft?.text, "local");
  await c.select("alpha", ref("3"));
  await c.select("alpha", ref());
  equal(c.state.draft?.conflict, true);
  equal(c.state.draft?.expectedMutationId, "9007199254740993");
  equal(mutations, 1);
});
Deno.test("Drive acknowledged save preserves edits made while the request was in flight", async () => {
  const pending = deferred<DriveMutationResponse>();
  const c = controller({ mutate: () => pending.promise });
  await c.select("alpha", ref());
  c.edit("sent");
  const saving = c.save();
  c.edit("new edit");
  pending.resolve({
    request_id: "req-1",
    entry: entry("2", "alpha", "9007199254740994"),
  });
  await saving;
  equal(c.state.draft?.text, "new edit");
  equal(c.state.draft?.baseText, "sent");
  equal(c.state.draft?.expectedMutationId, "9007199254740994");
});
Deno.test("Drive late mutation and upload success cannot affect a newly selected identity", async () => {
  for (const operation of ["mutation", "upload"] as const) {
    const pending = deferred<DriveMutationResponse>();
    const c = controller({
      mutate: () => pending.promise,
      upload: () => pending.promise,
    });
    await c.select("alpha", ref());
    c.edit("sent");
    const sending = operation === "mutation"
      ? c.save()
      : c.upload(new Blob(["abc"]), {
        operation: "update",
        id: ref(),
        expected_mutation_id: "9007199254740993",
        content_type: "text/plain",
      });
    await c.select("beta", ref("2", "beta"));
    pending.resolve({
      request_id: "req-1",
      entry: entry("2", "alpha", "9007199254740994"),
    });
    await sending;
    equal(c.state.workspaceId, "beta");
    equal(c.state.receipts, []);
    equal(c.state.draft?.expectedMutationId, "9007199254740993");
    await c.select("alpha", ref());
    equal(c.state.receipts[0].state, "unknown");
    equal(c.state.draft?.text, "sent");
    equal(c.state.draft?.expectedMutationId, "9007199254740993");
  }
});
Deno.test("Drive unknown upload reconciles by request id and never blindly retransmits", async () => {
  let uploads = 0;
  let statuses = 0;
  let committed = false;
  const c = controller({
    upload: () => {
      ++uploads;
      return Promise.reject(
        new DriveRequestError("outcome_unknown", "unknown"),
      );
    },
    status: (_, id) => {
      ++statuses;
      equal(id, "req-1");
      return Promise.resolve(
        committed
          ? {
            request_id: id,
            state: "committed",
            response: {
              request_id: id,
              entry: entry("2", "alpha", "9007199254740994"),
            },
          }
          : { request_id: id, state: "uncommitted", response: null },
      );
    },
  });
  await c.select("alpha", ref());
  const target = {
    operation: "update" as const,
    id: ref(),
    expected_mutation_id: "9007199254740993",
    content_type: "application/octet-stream",
  };
  const id = await c.upload(new Blob(["abc"]), target);
  equal(id, "req-1");
  equal(c.state.receipts[0].state, "unknown");
  await c.reconcile(id);
  equal(c.state.receipts[0].state, "unknown");
  await rejects(() => c.upload(new Blob(["abc"]), target));
  equal(uploads, 1);
  committed = true;
  await c.reconcile(id);
  equal(c.state.receipts[0].state, "committed");
  equal(uploads, 1);
  equal(statuses, 2);
});
Deno.test("Drive late status response is fenced even when the original node is reselected", async () => {
  const status = deferred<DriveRequestStatusResponse>();
  const c = controller({
    mutate: () =>
      Promise.reject(new DriveRequestError("outcome_unknown", "unknown")),
    status: () => status.promise,
  });
  await c.select("alpha", ref());
  c.edit("local");
  const id = await c.save();
  const checking = c.reconcile(id);
  await c.select("alpha", ref("3"));
  await c.select("alpha", ref());
  status.resolve({
    request_id: id,
    state: "committed",
    response: {
      request_id: id,
      entry: entry("2", "alpha", "9007199254740994"),
    },
  });
  await checking;
  equal(c.state.receipts[0].state, "unknown");
  equal(c.state.draft?.expectedMutationId, "9007199254740993");
  equal(c.state.draft?.text, "local");
});
Deno.test("Drive truncated text has no editable draft and disposal rejects late publication", async () => {
  const c = controller({
    readText: () =>
      Promise.resolve({ entry: entry(), text: "abc", truncated: true }),
  });
  await c.select("alpha", ref());
  equal(c.state.draft, null);
  await rejects(() => c.save());
  const late = deferred<DriveReadTextResponse>();
  const started = deferred<void>();
  const disposed = controller({
    readText: () => {
      started.resolve();
      return late.promise;
    },
  });
  let notifications = 0;
  disposed.subscribe(() => ++notifications);
  const selection = disposed.select("alpha", ref());
  await started.promise;
  disposed.dispose();
  const before = notifications;
  late.resolve({ entry: entry(), text: "late", truncated: false });
  await selection;
  equal(notifications, before);
  equal(disposed.state.text, null);
});
Deno.test("Drive search and pagination ignore results from the previous identity", async () => {
  const search = deferred<
    { entries: ReturnType<typeof entry>[]; next_after: string | null }
  >();
  const c = controller({ search: () => search.promise });
  await c.select("alpha", ref("1"));
  const searching = c.search("old");
  await c.select("beta", ref("1", "beta"));
  search.resolve({ entries: [entry()], next_after: "alpha:2" });
  await searching;
  equal(c.state.entries, []);
  equal(c.state.search, null);
  equal(c.state.nextAfter, null);
});
Deno.test("Drive late image preview does not publish into another workspace", async () => {
  const image = deferred<Blob>();
  const started = deferred<void>();
  const c = controller({
    metadata: (ws, target) =>
      Promise.resolve({
        ...entry(target.node_id, ws),
        content_type: "image/png",
      }),
    image: (ws) => {
      if (ws === "alpha") {
        started.resolve();
        return image.promise;
      }
      return Promise.resolve(new Blob(["new"]));
    },
  });
  const selecting = c.select("alpha", ref());
  await started.promise;
  await c.select("beta", ref("2", "beta"));
  const current = c.state.image;
  image.resolve(new Blob(["old"]));
  await selecting;
  assert(c.state.image === current);
});
Deno.test("Drive delayed initial list cannot overwrite a newer search in the same folder", async () => {
  const list = deferred<
    { entries: ReturnType<typeof entry>[]; next_after: string | null }
  >();
  const started = deferred<void>();
  const c = controller({
    list: () => {
      started.resolve();
      return list.promise;
    },
    search: () =>
      Promise.resolve({ entries: [entry("4")], next_after: "alpha:4" }),
  });
  const selecting = c.select("alpha", ref("1"));
  await started.promise;
  await c.search("new");
  list.resolve({ entries: [entry("2")], next_after: "alpha:2" });
  await selecting;
  equal(c.state.entries.map((e) => e.entry.node_id), ["4"]);
  equal(c.state.nextAfter, "alpha:4");
  equal(c.state.search?.query, "new");
});
Deno.test("Drive delayed root and metadata are fenced before publishing selection", async () => {
  for (const kind of ["root", "metadata"] as const) {
    const old = deferred<ReturnType<typeof entry>>();
    const c = controller({
      [kind]: (ws: string) =>
        ws === "alpha"
          ? old.promise
          : Promise.resolve(entry("1", ws, "1", "folder")),
    });
    const selecting = c.select(
      "alpha",
      kind === "metadata" ? ref("1") : undefined,
    );
    await c.select("beta", ref("1", "beta"));
    old.resolve(entry("1", "alpha", "1", "folder"));
    await selecting;
    equal(c.state.entry?.entry.workspace_id, "beta");
    equal(c.state.error, null);
  }
});
Deno.test("Drive pagination completion cannot publish after workspace switch", async () => {
  const late = deferred<
    { entries: ReturnType<typeof entry>[]; next_after: string | null }
  >();
  const c = controller({
    list: (ws, _, options) =>
      options?.after ? late.promise : Promise.resolve({
        entries: [],
        next_after: ws === "alpha" ? "alpha:2" : null,
      }),
  });
  await c.select("alpha", ref("1"));
  const loading = c.loadMore();
  await c.select("beta", ref("1", "beta"));
  late.resolve({ entries: [entry()], next_after: "alpha:3" });
  await loading;
  equal(c.state.entries, []);
  equal(c.state.nextAfter, null);
  equal(c.state.loading, false);
});
Deno.test("Drive late mutation conflict cannot mark the new node's draft", async () => {
  const late = deferred<DriveMutationResponse>();
  const c = controller({ mutate: () => late.promise });
  await c.select("alpha", ref());
  const saving = c.save();
  await c.select("alpha", ref("3"));
  late.reject(new DriveRequestError("conflict", "not_committed"));
  await saving;
  equal(c.state.draft?.conflict, false);
  equal(c.state.error, null);
});
Deno.test("Drive cancelled writes retain draft and receipt and ignore late completion after committed status", async () => {
  for (const operation of ["save", "upload"] as const) {
    const pending = deferred<DriveMutationResponse>();
    let writeSignal: AbortSignal | undefined;
    let statusSignal: AbortSignal | undefined;
    let statuses = 0;
    let sends = 0;
    const send = (
      _ws: string,
      _id: string,
      _body: unknown,
      signal?: AbortSignal,
    ) => {
      ++sends;
      writeSignal = signal;
      return pending.promise;
    };
    const c = controller({
      mutate: send,
      upload: (_ws, _id, _blob, _target, signal) =>
        send(_ws, _id, _blob, signal),
      status: (_ws, id, signal) => {
        statusSignal = signal;
        ++statuses;
        return Promise.resolve(
          statuses === 1
            ? { request_id: id, state: "uncommitted", response: null }
            : {
              request_id: id,
              state: "committed",
              response: {
                request_id: id,
                entry: entry("2", "alpha", "9007199254740994"),
              },
            },
        );
      },
    });
    await c.select("alpha", ref());
    c.edit("sent draft");
    const sending = operation === "save"
      ? c.save()
      : c.upload(new Blob(["abc"]), {
        operation: "update",
        id: ref(),
        expected_mutation_id: "9007199254740993",
        content_type: "text/plain",
      });
    equal(c.state.receipts[0].state, "pending");
    const id = c.state.receipts[0].requestId;
    c.cancelPending();
    assert(writeSignal?.aborted);
    equal(c.state.receipts[0].requestId, id);
    equal(c.state.receipts[0].state, "unknown");
    equal(c.state.draft?.text, "sent draft");
    equal(c.state.draft?.expectedMutationId, "9007199254740993");
    await rejects(() => c.save());
    await c.reconcile(id);
    assert(
      statusSignal && !statusSignal.aborted && statusSignal !== writeSignal,
    );
    equal(c.state.receipts[0].state, "unknown");
    await rejects(() =>
      c.upload(new Blob(["abc"]), {
        operation: "create",
        parent: ref("1"),
        name: "note.txt",
        content_type: "text/plain",
      })
    );
    await c.reconcile(id);
    equal(c.state.receipts[0].state, "committed");
    const snapshot = c.state;
    pending.resolve({
      request_id: id,
      entry: entry("2", "alpha", "9007199254740999"),
    });
    equal(await sending, id);
    assert(c.state === snapshot);
    equal(sends, 1);
    equal(c.state.entry?.last_mutation_id, "9007199254740994");
  }
});
Deno.test("Drive cancellation does not abort or fence active search reads", async () => {
  const search = deferred<
    { entries: ReturnType<typeof entry>[]; next_after: string | null }
  >();
  const upload = deferred<DriveMutationResponse>();
  let readSignal: AbortSignal | undefined;
  let writeSignal: AbortSignal | undefined;
  const c = controller({
    search: (_ws, _text, _include, _options, signal) => {
      readSignal = signal;
      return search.promise;
    },
    upload: (_ws, _id, _blob, _target, signal) => {
      writeSignal = signal;
      return upload.promise;
    },
  });
  await c.select("alpha", ref("1"));
  const searching = c.search("read still active");
  const uploading = c.upload(new Blob(["abc"]), {
    operation: "create",
    parent: ref("1"),
    name: "note.txt",
    content_type: "text/plain",
  });
  c.cancelPending();
  assert(writeSignal?.aborted);
  assert(readSignal && !readSignal.aborted);
  search.resolve({ entries: [entry("3")], next_after: null });
  await searching;
  equal(c.state.entries[0].entry.node_id, "3");
  equal(c.state.loading, false);
  upload.reject(new DriveRequestError("conflict", "not_committed"));
  await uploading;
  equal(c.state.receipts[0].state, "unknown");
  equal(c.state.error, null);
});
Deno.test("Drive concurrent writes are blocked before transmission and receipt creation", async () => {
  const pending = deferred<DriveMutationResponse>();
  let sends = 0;
  const c = controller({
    mutate: () => {
      ++sends;
      return pending.promise;
    },
    upload: () => {
      ++sends;
      return pending.promise;
    },
  });
  await c.select("alpha", ref());
  const saving = c.save();
  await rejects(() => c.save());
  await rejects(() =>
    c.mutate({
      operation: "delete",
      id: ref(),
      expected_mutation_id: "9007199254740993",
    })
  );
  await rejects(() =>
    c.upload(new Blob(["abc"]), {
      operation: "update",
      id: ref(),
      expected_mutation_id: "9007199254740993",
      content_type: "text/plain",
    })
  );
  equal(sends, 1);
  equal(c.state.receipts.length, 1);
  pending.resolve({
    request_id: "req-1",
    entry: entry("2", "alpha", "9007199254740994"),
  });
  await saving;
});
Deno.test("Drive clean draft refresh adopts latest last_mutation_id unless a request remains unresolved", async () => {
  let last_mutation_id = "9007199254740993";
  let text = "abc";
  const c = controller({
    readText: () =>
      Promise.resolve({
        entry: entry("2", "alpha", last_mutation_id),
        text,
        truncated: false,
      }),
    mutate: () =>
      Promise.reject(new DriveRequestError("outcome_unknown", "unknown")),
  });
  await c.select("alpha", ref());
  last_mutation_id = "9007199254740994";
  text = "new";
  await c.refresh();
  equal(c.state.draft?.text, "new");
  equal(c.state.draft?.baseText, "new");
  equal(c.state.draft?.expectedMutationId, last_mutation_id);
  await c.save();
  last_mutation_id = "9007199254740995";
  text = "unknown change";
  await c.refresh();
  equal(c.state.text, "unknown change");
  equal(c.state.draft?.text, "new");
  equal(c.state.draft?.expectedMutationId, "9007199254740994");
  equal(c.state.receipts[0].state, "unknown");
  await rejects(() => c.save());
});
Deno.test("Drive previews allow plain text markdown and four raster types only", async () => {
  for (
    const contentType of [
      "text/plain",
      "text/markdown",
      "image/png",
      "image/jpeg",
      "image/gif",
      "image/webp",
      "text/html",
      "text/javascript",
      "text/xml",
      "text/css",
      "application/json",
      "application/xml",
      "application/javascript",
      "image/svg+xml",
      "image/avif",
    ]
  ) {
    let textReads = 0;
    let imageReads = 0;
    const c = controller({
      metadata: () =>
        Promise.resolve({ ...entry(), content_type: contentType }),
      readText: () => {
        ++textReads;
        return Promise.resolve({
          entry: { ...entry(), content_type: contentType },
          text: "abc",
          truncated: false,
        });
      },
      image: () => {
        ++imageReads;
        return Promise.resolve(new Blob(["abc"]));
      },
    });
    await c.select("alpha", ref());
    equal(
      textReads,
      ["text/plain", "text/markdown"].includes(contentType) ? 1 : 0,
    );
    equal(
      imageReads,
      ["image/png", "image/jpeg", "image/gif", "image/webp"].includes(
          contentType,
        )
        ? 1
        : 0,
    );
    if (textReads === 0) {
      equal(c.state.text, null);
      equal(c.state.draft, null);
    }
  }
});
Deno.test("Drive cancelled late success stays unknown until explicit committed status lookup", async () => {
  const late = deferred<DriveMutationResponse>();
  let statusCalls = 0;
  const c = controller({
    mutate: () => late.promise,
    status: (_ws, id) => {
      ++statusCalls;
      return Promise.resolve({
        request_id: id,
        state: "committed",
        response: {
          request_id: id,
          entry: entry("2", "alpha", "9007199254740994"),
        },
      });
    },
  });
  await c.select("alpha", ref());
  c.edit("local");
  const saving = c.save();
  c.cancelPending();
  late.resolve({
    request_id: "req-1",
    entry: entry("2", "alpha", "9007199254740994"),
  });
  const id = await saving;
  equal(c.state.receipts[0].state, "unknown");
  equal(c.state.draft?.expectedMutationId, "9007199254740993");
  equal(c.state.draft?.text, "local");
  equal(statusCalls, 0);
  await rejects(() => c.save());
  await c.reconcile(id);
  equal(c.state.receipts[0].state, "committed");
  equal(c.state.draft?.expectedMutationId, "9007199254740994");
  equal(c.state.draft?.baseText, "local");
});
Deno.test("Drive archived dirty draft cannot enable editing after MIME changes to active bytes", async () => {
  let contentType = "text/plain";
  let textReads = 0;
  const c = controller({
    metadata: () => Promise.resolve({ ...entry(), content_type: contentType }),
    readText: () => {
      ++textReads;
      return Promise.resolve({
        entry: { ...entry(), content_type: contentType },
        text: "abc",
        truncated: false,
      });
    },
  });
  await c.select("alpha", ref());
  c.edit("retain me");
  contentType = "text/html";
  await c.refresh();
  equal(textReads, 1);
  equal(c.state.draft, null);
  equal(c.state.text, null);
  throws(() => c.edit("unsafe edit"));
  await rejects(() => c.save());
  contentType = "text/plain";
  await c.refresh();
  equal(c.state.draft?.text, "retain me");
  equal(c.state.draft?.expectedMutationId, "9007199254740993");
});
Deno.test("Drive conflicted clean draft stays fixed while clean truncated reread removes editor", async () => {
  let last_mutation_id = "9007199254740993";
  let truncated = false;
  const readText = () =>
    Promise.resolve({
      entry: entry("2", "alpha", last_mutation_id),
      text: "abc",
      truncated,
    });
  const conflict = controller({
    readText,
    mutate: () =>
      Promise.reject(new DriveRequestError("conflict", "not_committed")),
  });
  await conflict.select("alpha", ref());
  await conflict.save();
  last_mutation_id = "9007199254740994";
  await conflict.refresh();
  equal(conflict.state.draft?.text, "abc");
  equal(conflict.state.draft?.baseText, "abc");
  equal(conflict.state.draft?.conflict, true);
  equal(conflict.state.draft?.expectedMutationId, "9007199254740993");
  const clean = controller({ readText });
  await clean.select("alpha", ref());
  assert(clean.state.draft);
  truncated = true;
  await clean.refresh();
  equal(clean.state.draft, null);
  await rejects(() => clean.save());
});
