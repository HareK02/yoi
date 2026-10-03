// @vitest-environment happy-dom

import { cleanup, render } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import { createRawSnippet } from "svelte";
import Layout from "./+layout.svelte";

const resources = vi.hoisted(() => {
  const events: string[] = [];
  return {
    events,
    disposeMultiplexer: vi.fn((workspaceId: string) => {
      events.push(`multiplexer:${workspaceId}`);
    }),
    disposeWorkers: vi.fn((workspaceId: string) => {
      events.push(`workers:${workspaceId}`);
    }),
  };
});

vi.mock("$app/state", () => ({
  page: {
    params: { workspaceId: "workspace-a" },
    url: new URL("https://example.test/w/workspace-a"),
  },
}));
vi.mock("#lib/workspace/multiplexer.ts", () => ({
  disposeWorkspaceMultiplexer: resources.disposeMultiplexer,
}));
vi.mock("#lib/workspace/sidebar/worker-subscription.ts", () => ({
  disposeWorkspaceWorkersStore: resources.disposeWorkers,
  workspaceWorkersStore: () => ({
    subscribe: (
      subscriber: (state: { loading: boolean; workers: never[] }) => void,
    ) => {
      subscriber({ loading: false, workers: [] });
      return () => {};
    },
  }),
}));
vi.mock("#lib/workspace/sidebar/context.ts", () => ({
  SIDEBAR_CONTEXT: Symbol("test-sidebar"),
  getSidebarController: () => ({ registerSidebar: () => () => {} }),
}));
vi.mock("#lib/workspace/header/context.ts", () => ({
  getHeaderController: () => ({ registerContent: () => () => {} }),
}));

const children = createRawSnippet(() => ({ render: () => "<p>content</p>" }));
function data(workspaceId: string, displayName = workspaceId) {
  return {
    workspace: { workspace_id: workspaceId, display_name: displayName },
    workspaceError: null,
    repositories: [],
    repositoriesError: null,
  };
}

afterEach(() => {
  cleanup();
  resources.events.length = 0;
  resources.disposeMultiplexer.mockClear();
  resources.disposeWorkers.mockClear();
});

test("same-Workspace layout data refresh keeps resources, while route switch and exit dispose in order", async () => {
  const view = render(Layout, {
    props: { data: data("workspace-a"), children } as never,
  });
  expect(resources.events).toEqual([]);

  await view.rerender({
    data: data("workspace-a", "Updated display name"),
    children,
  } as never);
  expect(resources.events).toEqual([]);

  await view.rerender({ data: data("workspace-b"), children } as never);
  expect(resources.events).toEqual([
    "workers:workspace-a",
    "multiplexer:workspace-a",
  ]);

  view.unmount();
  expect(resources.events).toEqual([
    "workers:workspace-a",
    "multiplexer:workspace-a",
    "workers:workspace-b",
    "multiplexer:workspace-b",
  ]);
});
