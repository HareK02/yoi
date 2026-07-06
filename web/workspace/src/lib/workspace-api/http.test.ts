import { workspaceApiPath, workspaceRoute } from "./http.ts";

declare const Deno: {
  test(name: string, fn: () => void): void;
};

function assertEquals<T>(actual: T, expected: T): void {
  const actualJson = JSON.stringify(actual);
  const expectedJson = JSON.stringify(expected);
  if (actualJson !== expectedJson) {
    throw new Error(`Expected ${expectedJson}, got ${actualJson}`);
  }
}

Deno.test("workspace route helpers scope browser routes and API by immutable workspace id", () => {
  assertEquals(workspaceRoute("workspace 1"), "/w/workspace%201");
  assertEquals(workspaceRoute("workspace 1", "/objectives"), "/w/workspace%201/objectives");
  assertEquals(
    workspaceApiPath("workspace 1", "/repositories/repo-a"),
    "/api/w/workspace%201/repositories/repo-a",
  );
});
