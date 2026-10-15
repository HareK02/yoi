// @vitest-environment happy-dom

import type { AlertLevel, Event } from "#lib/generated/protocol.ts";
import { cleanup, render, waitFor } from "@testing-library/svelte";
import { afterEach, expect, test } from "vitest";
import { Window } from "happy-dom";
import { compile } from "svelte/compiler";
import ConsoleLineItem from "./ConsoleLineItem.svelte";
import componentSource from "./ConsoleLineItem.svelte?raw";
import { type ConsoleLine, createConsoleProjector } from "./model.ts";

afterEach(cleanup);

// Vitest does not attach extracted component CSS. Compile the real scoped CSS
// into an isolated style document so the assertions exercise the cascade.
function styledLine(container: HTMLElement, colorScheme: "light" | "dark") {
  const scope = [...container.querySelector("li")!.classList].find((name) =>
    name.startsWith("svelte-")
  )!;
  const componentCss = compile(componentSource, {
    filename: "ConsoleLineItem.svelte",
    cssHash: () => scope,
  }).css!.code;
  const browser = new Window({
    settings: { device: { prefersColorScheme: colorScheme } },
  });
  const style = browser.document.createElement("style");
  // Happy DOM cannot resolve app.css's @layer/oklch tokens. These sentinel
  // values verify that non-Alerts still use the unchanged semantic tokens.
  style.textContent =
    `:root { --danger: #cc3344; --text-muted: #777777; }\n${componentCss}`;
  browser.document.head.append(style);
  browser.document.body.innerHTML = container.innerHTML;
  return {
    heading: browser.getComputedStyle(
      browser.document.querySelector(".message-heading")!,
    ).color,
    body: browser.getComputedStyle(
      browser.document.querySelector(".console-plain-text")!,
    ).color,
  };
}

for (const colorScheme of ["light", "dark"] as const) {
  for (const level of ["warn", "error"] satisfies AlertLevel[]) {
    test(`${level} Alert has an orange heading and readable grey body in ${colorScheme}`, () => {
      const item: ConsoleLine = {
        id: `alert-${level}`,
        kind: level === "error" ? "error" : "status",
        title: "Alert · engine",
        body: "The engine needs attention.",
        source: "event",
        error: level === "error",
        alert: {
          level,
          source: "engine",
          message: "The engine needs attention.",
          timestamp_ms: 1,
        },
      };
      const { container } = render(ConsoleLineItem, { item });
      expect(container.querySelector("li")?.classList.contains("alert-line"))
        .toBe(true);
      expect(container.querySelector(".message-heading")?.textContent).toBe(
        item.title,
      );
      expect(container.querySelector(".console-plain-text")?.textContent).toBe(
        item.body,
      );
      expect(styledLine(container, colorScheme)).toEqual({
        heading: colorScheme === "light" ? "#a74709" : "#f6a75c",
        body: colorScheme === "light" ? "#595959" : "#b8b8b8",
      });
    });
  }

  test(`non-Alert errors retain error styling in ${colorScheme}, even with an Alert-like title`, () => {
    const { container } = render(ConsoleLineItem, {
      item: {
        id: "ordinary-error",
        kind: "error",
        title: "Alert · engine",
        body: "An ordinary error, not a protocol Alert.",
        source: "event",
        error: true,
      },
    });
    expect(container.querySelector("li")?.classList.contains("alert-line"))
      .toBe(false);
    expect(container.querySelector("li")?.classList.contains("error-line"))
      .toBe(true);
    expect(styledLine(container, colorScheme)).toEqual({
      heading: "#777777",
      body: "#cc3344",
    });
  });
}

function reconnectSnapshot(text: string): Event {
  return {
    event: "snapshot",
    data: {
      session: {
        entries: [],
        pending_submissions: {
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
