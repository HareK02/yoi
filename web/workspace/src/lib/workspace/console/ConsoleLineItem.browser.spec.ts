// @vitest-environment happy-dom

import type { Event } from "#lib/generated/protocol.ts";
import { cleanup, render, waitFor } from "@testing-library/svelte";
import { afterEach, expect, test } from "vitest";
import ConsoleLineItem from "./ConsoleLineItem.svelte";
import { createConsoleProjector } from "./model.ts";

afterEach(cleanup);

function reconnectSnapshot(text: string): Event {
  return {
    event: "snapshot",
    data: {
      session: {
        entries: [],
        pending_submissions: {
          revision: 0,
          head_id: null,
          notification_count: 0,
          submissions: [],
        },
      },
      greeting: {
        worker_name: "Worker",
        cwd: "/repo",
        provider: "provider",
        model: "model",
        scope_summary: "bounded",
        tools: [],
        context_window: 100,
        context_tokens: 20,
      },
      state: {
        last_command_id: 0,
        state: { kind: "busy", state: { kind: "run", state: "running" } },
      },
      in_flight: {
        blocks: [{ kind: "text", text, finished: false }],
      },
    },
  };
}

test("reconnected assistant output keeps the normal streaming Markdown path", async () => {
  const projector = createConsoleProjector();
  let projection = projector.append([{
    eventId: "snapshot",
    event: reconnectSnapshot("**hel"),
  }]);
  const { container, rerender } = render(ConsoleLineItem, {
    item: projection.lines[0],
  });

  expect(container.querySelector("li")?.classList.contains("assistant")).toBe(
    true,
  );
  expect(container.querySelector(".message-heading")).toBeNull();
  expect(container.querySelector(".console-plain-text")).toBeNull();
  expect(container.textContent).not.toContain("in-flight");

  projection = projector.append([{
    eventId: "inline-suffix",
    event: {
      event: "text_delta",
      data: { text: "lo** and [safe](https://exam" },
    },
  }]);
  await rerender({ item: projection.lines[0] });
  await waitFor(() => {
    expect(container.querySelector("strong")?.textContent).toBe("hello");
    expect(container.querySelector('a[href="https://example.com"]')).toBeNull();
    expect(container.textContent).toContain("[safe](https://exam");
  });

  projection = projector.append([{
    eventId: "fence-prefix",
    event: {
      event: "text_delta",
      data: { text: "ple.com)\n\n```ts\nconst answer = 42;" },
    },
  }]);
  await rerender({ item: projection.lines[0] });
  await waitFor(() => {
    expect(container.querySelector('a[href="https://example.com"]')).not
      .toBeNull();
    expect(container.querySelector("pre code")?.textContent).toContain(
      "const answer = 42;",
    );
  });

  projection = projector.append([{
    eventId: "fence-suffix",
    event: { event: "text_delta", data: { text: "\n```" } },
  }]);
  await rerender({ item: projection.lines[0] });
  await waitFor(() => {
    expect(container.querySelector("strong")?.textContent).toBe("hello");
    expect(container.querySelector('a[href="https://example.com"]')).not
      .toBeNull();
    expect(container.querySelector("pre code")?.textContent).toContain(
      "const answer = 42;",
    );
  });

  expect(projection.lines).toHaveLength(1);
  expect(projection.lines[0]).toMatchObject({
    kind: "assistant",
    streaming: true,
    body:
      "**hello** and [safe](https://example.com)\n\n```ts\nconst answer = 42;\n```",
  });
});
