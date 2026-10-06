// @vitest-environment happy-dom

import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";

const { gotoMock } = vi.hoisted(() => ({
  gotoMock: vi.fn(async () => {}),
}));

vi.mock("$app/navigation", () => ({ goto: gotoMock }));

import SubjectIndexPage from "./+page.svelte";
import SubjectPage from "./[subjectId]/+page.svelte";
import MemoryDetailPage from "./[subjectId]/[memoryId]/+page.svelte";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

const result = <T>(data: T | null, error: string | null = null) => ({
  data,
  error,
});

function subject() {
  return {
    id: "subject-1",
    role: "Release coordinator",
    behavior_md: "Prefer explicit evidence.",
    behavior_revision: 2,
    state: "active" as const,
    store_revision: 9,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-02T03:04:05Z",
  };
}

function subjectIndexData(items = [subject()]) {
  return {
    workspaceId: "workspace-1",
    cursor: null,
    subjects: result({
      limit: 100,
      items,
      has_more: false,
    }),
  };
}

test("renders the subject index as the Memory product entry", () => {
  render(SubjectIndexPage, {
    data: {
      workspaceId: "workspace-1",
      cursor: "subject-current",
      subjects: result({
        limit: 100,
        items: [subject()],
        next_cursor: "subject-next",
        has_more: true,
      }),
    },
  } as never);

  expect(screen.getByRole("heading", { name: "Subjects", level: 1 })).not
    .toBeNull();
  expect(screen.getByRole("button", { name: "New Subject" })).not.toBeNull();
  expect(screen.getByRole("link", { name: /Release coordinator/ })).not
    .toBeNull();
  expect(screen.getByText("subject-1")).not.toBeNull();
  expect(screen.getByText("Not connected")).not.toBeNull();
  expect(screen.queryByText("Store revision", { exact: true })).toBeNull();
  expect(screen.getByText("1 shown · more available")).not.toBeNull();
  expect(screen.getByRole("link", { name: "First page" }).getAttribute("href"))
    .toBe("/w/workspace-1/memory");
  expect(screen.getByRole("link", { name: /Next page/ }).getAttribute("href"))
    .toContain("cursor=subject-next");
  expect(screen.queryByText("Staging")).toBeNull();
});

test("creates one Subject with the exact role and navigates to its generated detail", async () => {
  let resolveRequest!: (response: Response) => void;
  const response = new Promise<Response>((resolve) => {
    resolveRequest = resolve;
  });
  const fetchMock = vi.fn(() => response);
  vi.stubGlobal("fetch", fetchMock);
  render(SubjectIndexPage, { data: subjectIndexData() } as never);

  const trigger = screen.getByRole("button", { name: "New Subject" });
  await fireEvent.click(trigger);
  const roleInput = screen.getByRole("textbox", {
    name: "Role",
  }) as HTMLInputElement;
  await waitFor(() => expect(document.activeElement).toBe(roleInput));
  await fireEvent.input(roleInput, { target: { value: "  Review lead  " } });
  const behaviorInput = screen.getByRole("textbox", {
    name: /Behavior/,
  }) as HTMLTextAreaElement;
  await fireEvent.input(behaviorInput, {
    target: { value: "Challenge unsupported assumptions." },
  });

  const submit = screen.getByRole("button", { name: "Create Subject" });
  await fireEvent.click(submit);
  await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));
  expect(
    screen.getByRole("button", { name: "Creating…" }).hasAttribute("disabled"),
  )
    .toBe(true);
  await fireEvent.click(screen.getByRole("button", { name: "Creating…" }));
  expect(fetchMock).toHaveBeenCalledTimes(1);

  resolveRequest(Response.json({
    id: "generated-subject-1",
    role: "  Review lead  ",
    behavior_md: "Challenge unsupported assumptions.",
    behavior_revision: 0,
    state: "active",
    store_revision: 0,
    created_at: "2026-01-03T00:00:00Z",
    updated_at: "2026-01-03T00:00:00Z",
  }, { status: 201 }));
  await waitFor(() => {
    expect(gotoMock).toHaveBeenCalledWith(
      "/w/workspace-1/memory/generated-subject-1",
    );
  });

  const [path, init] = fetchMock.mock.calls[0] as unknown as [
    string,
    RequestInit,
  ];
  expect(path).toBe("/api/w/workspace-1/subjektiv/subjects");
  expect(init.method).toBe("POST");
  expect(init.body).toBe(
    '{"role":"  Review lead  ","behavior_md":"Challenge unsupported assumptions."}',
  );
});

test("keeps creation available for an empty list and restores focus on cancel", async () => {
  vi.stubGlobal("fetch", vi.fn());
  render(SubjectIndexPage, { data: subjectIndexData([]) } as never);

  expect(screen.getByText("No Memory subjects.")).not.toBeNull();
  const trigger = screen.getByRole("button", { name: "New Subject" });
  await fireEvent.click(trigger);
  const input = screen.getByRole("textbox", {
    name: "Role",
  }) as HTMLInputElement;
  await fireEvent.input(input, { target: { value: "Draft role" } });
  await fireEvent.keyDown(input, { key: "Escape" });
  await waitFor(() => expect(document.activeElement).toBe(trigger));
  expect(screen.queryByRole("textbox", { name: "Role" })).toBeNull();
});

test("preserves invalid and rejected Subject drafts with accessible field errors", async () => {
  const fetchMock = vi.fn(() =>
    Promise.resolve(Response.json(
      {
        error: "Forbidden",
        message: "workspace permission denied",
        diagnostics: [],
      },
      { status: 403 },
    ))
  );
  vi.stubGlobal("fetch", fetchMock);
  render(SubjectIndexPage, { data: subjectIndexData() } as never);

  await fireEvent.click(screen.getByRole("button", { name: "New Subject" }));
  const input = screen.getByRole("textbox", {
    name: "Role",
  }) as HTMLInputElement;
  await fireEvent.input(input, { target: { value: "   " } });
  await fireEvent.click(screen.getByRole("button", { name: "Create Subject" }));
  expect(screen.getByRole("alert").textContent).toContain("must not be empty");
  expect(input.getAttribute("aria-invalid")).toBe("true");
  expect(input.value).toBe("   ");
  expect(fetchMock).not.toHaveBeenCalled();

  await fireEvent.input(input, { target: { value: "Release reviewer" } });
  const behaviorInput = screen.getByRole("textbox", {
    name: /Behavior/,
  }) as HTMLTextAreaElement;
  await fireEvent.input(behaviorInput, { target: { value: "   " } });
  await fireEvent.click(screen.getByRole("button", { name: "Create Subject" }));
  expect(screen.getByRole("alert").textContent).toContain(
    "empty or contain non-whitespace",
  );
  expect(behaviorInput.getAttribute("aria-invalid")).toBe("true");
  expect(fetchMock).not.toHaveBeenCalled();

  await fireEvent.input(behaviorInput, {
    target: { value: "Challenge unsupported assumptions." },
  });
  await fireEvent.click(screen.getByRole("button", { name: "Create Subject" }));
  await waitFor(() => {
    expect(screen.getByRole("alert").textContent).toContain(
      "do not have permission",
    );
  });
  expect(input.value).toBe("Release reviewer");
  expect(input.getAttribute("aria-invalid")).toBeNull();
  expect(fetchMock).toHaveBeenCalledTimes(1);
});

test("warns about an unknown create outcome without replaying the POST", async () => {
  const fetchMock = vi.fn(() =>
    Promise.reject(new TypeError("connection reset"))
  );
  vi.stubGlobal("fetch", fetchMock);
  render(SubjectIndexPage, { data: subjectIndexData() } as never);

  await fireEvent.click(screen.getByRole("button", { name: "New Subject" }));
  const input = screen.getByRole("textbox", {
    name: "Role",
  }) as HTMLInputElement;
  await fireEvent.input(input, { target: { value: "Incident coordinator" } });
  await fireEvent.click(screen.getByRole("button", { name: "Create Subject" }));
  await waitFor(() => {
    expect(screen.getByRole("alert").textContent).toContain(
      "may have been created",
    );
  });
  expect(screen.getByRole("link", { name: /Check the first Subjects page/ }))
    .not.toBeNull();
  expect(input.value).toBe("Incident coordinator");
  expect(fetchMock).toHaveBeenCalledTimes(1);
  expect(gotoMock).not.toHaveBeenCalled();
});

test("renders a ready resident surface and committed Memory lifecycle states", async () => {
  render(SubjectPage, {
    data: {
      workspaceId: "workspace-1",
      subjectId: "subject-1",
      cursor: "memory-current",
      subject: result({
        ...subject(),
        current_worker: { display_name: "Release Worker" },
      }),
      surface: result({
        subject_id: "subject-1",
        availability: "ready" as const,
        snapshot: {
          snapshot_id: "snapshot-1",
          body_md:
            "# Resident context\n\nUse the **current** committed record.",
          memory_refs: [{ memory_id: "memory-1", revision: 3 }],
          built_from_store_revision: 9,
          created_at: "2026-01-02T03:04:05Z",
        },
      }),
      memories: result({
        items: [
          {
            id: "memory-1",
            revision: 3,
            kind: "decision" as const,
            state: "active" as const,
            claim: "Current decision",
            excerpt: "Current excerpt",
            updated_at: "2026-01-02T03:04:05Z",
          },
          {
            id: "memory-2",
            revision: 2,
            kind: "lesson" as const,
            state: "resolved" as const,
            claim: "Resolved lesson",
            excerpt: "Resolved excerpt",
            updated_at: "2026-01-02T03:04:05Z",
          },
          {
            id: "memory-3",
            revision: 1,
            kind: "constraint" as const,
            state: "retracted" as const,
            claim: "Retracted constraint",
            excerpt: "Retracted excerpt",
            updated_at: "2026-01-02T03:04:05Z",
          },
        ],
        next_cursor: "memory-next",
        has_more: true,
      }),
    },
  } as never);

  await waitFor(() => {
    expect(screen.getByRole("heading", { name: "Resident context", level: 1 }))
      .not.toBeNull();
  });
  expect(
    screen.getByRole("article", { name: "User-managed Subject behavior" })
      .textContent,
  )
    .toContain("Prefer explicit evidence.");
  expect(screen.getByRole("article", { name: "Resident context" })).not
    .toBeNull();
  expect(screen.getByText("Connected", { exact: true })).not.toBeNull();
  expect(screen.getByText("Release Worker", { exact: true })).not.toBeNull();
  expect(screen.getByText("3 on this page", { exact: true })).not.toBeNull();
  expect(screen.getByText("Subject store revision", { exact: true })).not
    .toBeNull();
  expect(screen.getByText(/not a Memory count/)).not.toBeNull();
  expect(screen.getAllByText("Active", { exact: true }).length).toBeGreaterThan(
    0,
  );
  expect(screen.getByText("Resolved", { exact: true })).not.toBeNull();
  expect(screen.getByText("Retracted", { exact: true })).not.toBeNull();
  expect(screen.getByRole("navigation", { name: "Current Memory pages" }))
    .not.toBeNull();
  expect(screen.getByRole("link", { name: /Next page/ }).getAttribute("href"))
    .toContain("cursor=memory-next");
});

test("edits and clears user-managed behavior with CAS while distinguishing save from Worker application", async () => {
  const fetchMock = vi.fn(async (_path: string, init?: RequestInit) => {
    const request = JSON.parse(String(init?.body));
    return Response.json({
      ...subject(),
      behavior_md: request.behavior_md,
      behavior_revision: request.expected_behavior_revision + 1,
    });
  });
  vi.stubGlobal("fetch", fetchMock);
  render(SubjectPage, {
    data: {
      workspaceId: "workspace-1",
      subjectId: "subject-1",
      subject: result(subject()),
      surface: result({
        subject_id: "subject-1",
        availability: "ungenerated" as const,
      }),
      memories: result({ items: [], has_more: false }),
    },
  } as never);

  await fireEvent.click(screen.getByRole("button", { name: "Edit" }));
  const textarea = screen.getByRole("textbox", { name: "Subject behavior" });
  await fireEvent.input(textarea, {
    target: { value: "Ask before irreversible actions." },
  });
  await fireEvent.click(screen.getByRole("button", { name: "Save behavior" }));
  await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));
  const [path, init] = fetchMock.mock.calls[0] as unknown as [
    string,
    RequestInit,
  ];
  expect(path).toBe("/api/w/workspace-1/subjektiv/subjects/subject-1/behavior");
  expect(init.method).toBe("PATCH");
  expect(init.body).toBe(
    '{"expected_behavior_revision":2,"behavior_md":"Ask before irreversible actions."}',
  );
  await waitFor(() => {
    expect(
      screen.getByText(/Behavior storage confirmed at revision 3/).textContent,
    )
      .toContain("does not confirm application");
  });
  expect(
    screen.getByRole("article", { name: "User-managed Subject behavior" })
      .textContent,
  )
    .toContain("Ask before irreversible actions.");

  await fireEvent.click(screen.getByRole("button", { name: "Edit" }));
  await fireEvent.click(screen.getByRole("button", { name: "Clear" }));
  await fireEvent.click(screen.getByRole("button", { name: "Save behavior" }));
  await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));
  const secondBody =
    (fetchMock.mock.calls[1] as unknown as [string, RequestInit])[1].body;
  expect(secondBody).toBe('{"expected_behavior_revision":3,"behavior_md":""}');
  await waitFor(() =>
    expect(screen.getByText("No behavior is set.")).not.toBeNull()
  );
});

function subjectPageData(record = subject(), workspaceId = "workspace-1") {
  return {
    workspaceId,
    subjectId: record.id,
    subject: result(record),
    surface: result({
      subject_id: record.id,
      availability: "ungenerated" as const,
    }),
    memories: result({ items: [], has_more: false }),
  };
}

test.each([
  ["subject-2", "workspace-1"],
  ["subject-1", "workspace-2"],
])(
  "resets reused Subject editor for %s in %s and saves only its own behavior",
  async (id, workspaceId) => {
    const nextSubject = { ...subject(), id, behavior_md: "Only B behavior." };
    const fetchMock = vi.fn(async (_path: string, init?: RequestInit) => {
      const request = JSON.parse(String(init?.body));
      return Response.json({
        ...nextSubject,
        behavior_md: request.behavior_md,
        behavior_revision: request.expected_behavior_revision + 1,
      });
    });
    vi.stubGlobal("fetch", fetchMock);
    const view = render(SubjectPage, { data: subjectPageData() } as never);
    await fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    await fireEvent.input(
      screen.getByRole("textbox", { name: "Subject behavior" }),
      {
        target: { value: "Unsaved A draft" },
      },
    );

    await view.rerender(
      { data: subjectPageData(nextSubject, workspaceId) } as never,
    );
    expect(screen.queryByRole("textbox", { name: "Subject behavior" }))
      .toBeNull();
    expect(
      screen.getByRole("article", { name: "User-managed Subject behavior" })
        .textContent,
    )
      .toContain("Only B behavior.");
    await fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    expect(
      (screen.getByRole("textbox", {
        name: "Subject behavior",
      }) as HTMLTextAreaElement).value,
    )
      .toBe("Only B behavior.");
    await fireEvent.click(
      screen.getByRole("button", { name: "Save behavior" }),
    );
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));
    const [path, init] = fetchMock.mock.calls[0] as unknown as [
      string,
      RequestInit,
    ];
    expect(path).toBe(
      `/api/w/${workspaceId}/subjektiv/subjects/${id}/behavior`,
    );
    expect(JSON.parse(String(init.body))).toEqual({
      expected_behavior_revision: 2,
      behavior_md: "Only B behavior.",
    });
    await waitFor(() =>
      expect(screen.getByText(/storage confirmed at revision 3/)).not.toBeNull()
    );
  },
);

test.each(["success", "failure"])(
  "fences late %s from a previous Subject without clearing the new save state",
  async (outcome) => {
    const pending: Array<(response: Response) => void> = [];
    const fetchMock = vi.fn(() =>
      new Promise<Response>((resolve) => pending.push(resolve))
    );
    vi.stubGlobal("fetch", fetchMock);
    const view = render(SubjectPage, { data: subjectPageData() } as never);
    await fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    await fireEvent.click(
      screen.getByRole("button", { name: "Save behavior" }),
    );
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));

    const nextSubject = {
      ...subject(),
      id: "subject-2",
      behavior_md: "Only B behavior.",
    };
    await view.rerender({ data: subjectPageData(nextSubject) } as never);
    await fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    await fireEvent.input(
      screen.getByRole("textbox", { name: "Subject behavior" }),
      {
        target: { value: "Updated B behavior." },
      },
    );
    await fireEvent.click(
      screen.getByRole("button", { name: "Save behavior" }),
    );
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));
    pending[0](
      outcome === "success"
        ? Response.json({
          ...subject(),
          behavior_md: "Late A response",
          behavior_revision: 3,
        })
        : Response.json({
          error: "Forbidden",
          message: "A denied",
          diagnostics: [],
        }, { status: 403 }),
    );
    // Drain the previous response through its success/catch/finally branches.
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(
      screen.getByRole("button", { name: "Saving…" }).hasAttribute("disabled"),
    ).toBe(true);
    expect(
      (screen.getByRole("textbox", {
        name: "Subject behavior",
      }) as HTMLTextAreaElement).value,
    )
      .toBe("Updated B behavior.");
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.queryByText(/storage confirmed/)).toBeNull();
    pending[1](
      Response.json({
        ...nextSubject,
        behavior_md: "Updated B behavior.",
        behavior_revision: 3,
      }),
    );
    await waitFor(() =>
      expect(screen.getByText(/storage confirmed at revision 3/)).not.toBeNull()
    );
    expect(
      screen.getByRole("article", { name: "User-managed Subject behavior" })
        .textContent,
    )
      .toContain("Updated B behavior.");
  },
);

test("a refreshed loader snapshot fences old saves even after returning to the same Subject", async () => {
  let resolveRequest!: (response: Response) => void;
  vi.stubGlobal(
    "fetch",
    vi.fn(() =>
      new Promise<Response>((resolve) => {
        resolveRequest = resolve;
      })
    ),
  );
  const view = render(SubjectPage, { data: subjectPageData() } as never);
  await fireEvent.click(screen.getByRole("button", { name: "Edit" }));
  await fireEvent.click(screen.getByRole("button", { name: "Save behavior" }));
  await view.rerender(
    { data: subjectPageData({ ...subject(), id: "subject-2" }) } as never,
  );
  const fresh = {
    ...subject(),
    behavior_md: "Fresh A loader",
    behavior_revision: 4,
  };
  await view.rerender({ data: subjectPageData(fresh) } as never);
  resolveRequest(
    Response.json({
      ...subject(),
      behavior_md: "Stale A response",
      behavior_revision: 3,
    }),
  );
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(
    screen.getByRole("article", { name: "User-managed Subject behavior" })
      .textContent,
  )
    .toContain("Fresh A loader");
  expect(screen.queryByText(/storage confirmed/)).toBeNull();

  await view.rerender(
    {
      data: {
        ...subjectPageData(),
        subject: result(null, "Subject unavailable"),
      },
    } as never,
  );
  expect(screen.queryByRole("button", { name: "Edit" })).toBeNull();
  expect(
    screen.queryByRole("article", { name: "User-managed Subject behavior" }),
  ).toBeNull();
});

test("candidate-only Subjects show no committed Memories, not an inferred candidate count", () => {
  // The store regression candidate_decisions_reject_conflicts_and_roll_back_partial_writes
  // stages a candidate without committing a Memory. Its public read projection is
  // the same empty committed list as a new Subject; candidates are not a page input.
  render(SubjectPage, {
    data: {
      workspaceId: "workspace-1",
      subjectId: "subject-1",
      cursor: null,
      subject: result({ ...subject(), store_revision: 0 }),
      surface: result({ subject_id: "subject-1", availability: "ungenerated" }),
      memories: result({ limit: 100, items: [], has_more: false }),
    },
  } as never);

  expect(screen.getByText("None", { exact: true })).not.toBeNull();
  expect(screen.getByText("No committed Memories yet.")).not.toBeNull();
  expect(screen.getByText("Not connected", { exact: true })).not.toBeNull();
  expect(screen.getAllByText("Not generated", { exact: true })).toHaveLength(2);
  expect(screen.queryByRole("article", { name: "Resident context" }))
    .toBeNull();
  expect(
    screen.queryByText(
      /candidate count|all Memories lost|automatically updating/i,
    ),
  ).toBeNull();
});

test.each(["ready", "ungenerated", "stale", "failed"] as const)(
  "user behavior and diagnostic disclosures stay independent of %s generated Memory",
  async (availability) => {
    const record = { ...subject(), store_revision: 0 };
    const view = render(SubjectPage, {
      data: {
        ...subjectPageData(record),
        surface: result({
          subject_id: record.id,
          availability,
          ...(availability === "ready"
            ? {
              snapshot: {
                snapshot_id: "empty-snapshot",
                body_md: "",
                memory_refs: [],
                built_from_store_revision: 0,
                created_at: "2026-01-02T03:04:05Z",
              },
            }
            : {}),
        }),
      },
    } as never);
    expect(
      screen.getByRole("article", { name: "User-managed Subject behavior" })
        .textContent,
    )
      .toContain(record.behavior_md);
    expect(screen.getByText("Generated Memory context", { exact: true })).not
      .toBeNull();
    expect(screen.getByText("No committed Memories yet.")).not.toBeNull();
    expect(screen.getByText("Not connected", { exact: true })).not.toBeNull();
    const technical = view.container.querySelector(
      "details.subject-technical-details",
    ) as HTMLDetailsElement;
    expect(technical.open).toBe(false);
    expect(technical.textContent).toContain("Subject store revision");
    if (availability === "ready") {
      expect(screen.getByText("Resident context is current but empty.")).not
        .toBeNull();
      expect(screen.getByText(/no generated Memory context to show/)).not
        .toBeNull();
      expect(
        view.container.querySelector("details.surface-details")?.textContent,
      )
        .toContain("Generated from Subject revision");
    }
    await fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(
      screen.getByRole("article", { name: "User-managed Subject behavior" })
        .textContent,
    )
      .toContain(record.behavior_md);
    expect(technical.open).toBe(false);
  },
);

test("distinguishes ready-empty, stale, failed, unavailable, and request error surfaces", async () => {
  const base = {
    workspaceId: "workspace-1",
    subjectId: "subject-1",
    subject: result(subject()),
    memories: result({ items: [], has_more: false }),
  };
  const view = render(SubjectPage, {
    data: {
      ...base,
      surface: result({
        subject_id: "subject-1",
        availability: "ready" as const,
        snapshot: {
          snapshot_id: "snapshot-empty",
          body_md: "",
          memory_refs: [],
          built_from_store_revision: 9,
          created_at: "2026-01-02T03:04:05Z",
        },
      }),
    },
  } as never);
  expect(screen.getByText("Resident context is current but empty.")).not
    .toBeNull();
  expect(screen.getByText(/current empty surface exists/)).not.toBeNull();
  expect(screen.getByText("No committed Memories yet.")).not.toBeNull();

  await view.rerender(
    {
      data: {
        ...base,
        surface: result({
          subject_id: "subject-1",
          availability: "ungenerated" as const,
        }),
      },
    } as never,
  );
  expect(screen.getByText("Resident context has not been generated.")).not
    .toBeNull();
  expect(screen.getAllByText("Not generated", { exact: true })).toHaveLength(2);

  await view.rerender(
    {
      data: {
        ...base,
        surface: result({
          subject_id: "subject-1",
          availability: "stale" as const,
        }),
      },
    } as never,
  );
  expect(screen.getByText("Resident context needs to be refreshed.")).not
    .toBeNull();

  await view.rerender(
    {
      data: {
        ...base,
        surface: result({
          subject_id: "subject-1",
          availability: "failed" as const,
        }),
      },
    } as never,
  );
  expect(screen.getByRole("alert").textContent).toContain("generation failed");

  await view.rerender({ data: { ...base, surface: result(null) } } as never);
  expect(screen.getByText("Resident context status is unavailable.")).not
    .toBeNull();

  await view.rerender(
    {
      data: { ...base, surface: result(null, "Memory API request failed") },
    } as never,
  );
  expect(screen.getByRole("alert").textContent).toContain(
    "Resident context status unavailable",
  );
});

test("renders committed Memory detail, provenance, derivation, and immutable revisions", async () => {
  render(MemoryDetailPage, {
    data: {
      workspaceId: "workspace-1",
      subjectId: "subject-1",
      memoryId: "memory-1",
      revisionCursor: "revision-current",
      subject: result(subject()),
      memory: result({
        memory_id: "memory-1",
        revision: 2,
        current_revision: 3,
        kind: "decision" as const,
        state: "resolved" as const,
        claim: "Keep provenance typed",
        body_md: "# Committed detail\n\nRead-only body.",
        why_useful: "Prevents origin loss.",
        change_reason: "Resolved after implementation.",
        created_at: "2026-01-01T00:00:00Z",
        updated_at: "2026-01-02T03:04:05Z",
        body_offset: 0,
        body_byte_offset: 0,
        body_truncated: false,
        source_candidate_ids: ["candidate-1"],
        source_candidates: [{
          candidate_id: "candidate-1",
          evidence: [{
            id: "evidence-1",
            kind: "message",
            summary: "Explicit decision.",
          }],
          evidence_total: 1,
          evidence_truncated: false,
          source_refs: [{
            session_id: "session-1",
            segment_id: "segment-1",
            evidence_id: "evidence-1",
            label: "Decision discussion",
          }],
          source_refs_total: 1,
          source_refs_truncated: false,
        }],
        derived_from: [{ memory_id: "memory-parent", revision: 4 }],
        evidence_has_more: false,
      }),
      revisions: result({
        memory_id: "memory-1",
        current_revision: 3,
        items: [{
          revision: 3,
          kind: "decision" as const,
          state: "active" as const,
          claim: "Current claim",
          change_reason: "Clarified",
          updated_at: "2026-01-03T00:00:00Z",
        }, {
          revision: 2,
          kind: "decision" as const,
          state: "resolved" as const,
          claim: "Earlier claim",
          change_reason: "Resolved",
          updated_at: "2026-01-02T00:00:00Z",
        }],
        next_cursor: "revision-next",
        has_more: true,
      }),
    },
  } as never);

  await waitFor(() => {
    expect(screen.getByRole("heading", { name: "Committed detail", level: 1 }))
      .not.toBeNull();
  });
  expect(screen.getByText("candidate-1")).not.toBeNull();
  expect(screen.getByText("Decision discussion")).not.toBeNull();
  expect(screen.getByText("memory-parent")).not.toBeNull();
  expect(screen.getByRole("heading", { name: "Revision history" })).not
    .toBeNull();
  expect(screen.getByText("Historical revision")).not.toBeNull();
  expect(screen.getByRole("navigation", { name: "Revision history pages" }))
    .not.toBeNull();
  expect(screen.getByRole("link", { name: /Next page/ }).getAttribute("href"))
    .toContain("revision_cursor=revision-next");
});
