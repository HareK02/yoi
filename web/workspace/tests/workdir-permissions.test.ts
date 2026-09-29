declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

import { formatWorkdirPermissions } from "../src/lib/workspace/settings/workdir-permissions.ts";

Deno.test("Workdir permissions render independent categories in stable order", () => {
  const cases = [
    [{ read: true, write: false, command: false }, "READ"],
    [{ read: true, write: true, command: false }, "READ · WRITE"],
    [{ read: false, write: false, command: true }, "COMMAND"],
    [{ read: true, write: false, command: true }, "READ · COMMAND"],
    [{ read: true, write: true, command: true }, "READ · WRITE · COMMAND"],
  ] as const;

  for (const [permissions, expected] of cases) {
    const actual = formatWorkdirPermissions(permissions);
    if (actual !== expected) {
      throw new Error(`expected ${expected}, got ${actual}`);
    }
  }
});
