declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

import {
  formatExternalGrantPermissionLevel,
  formatWorkdirPermissions,
} from "../src/lib/workspace/settings/workdir-permissions.ts";

Deno.test("effective Workdir permissions render partial intersections without widening", () => {
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

Deno.test("External grant levels show inherited authority and reject partial grants", () => {
  const cases = [
    [{ read: true, write: false, command: false }, "READ"],
    [{ read: true, write: true, command: false }, "WRITE (includes READ)"],
    [
      { read: true, write: true, command: true },
      "COMMAND (includes WRITE + READ)",
    ],
  ] as const;
  for (const [permissions, expected] of cases) {
    const actual = formatExternalGrantPermissionLevel(permissions);
    if (actual !== expected) {
      throw new Error(`expected ${expected}, got ${actual}`);
    }
  }

  let rejected = false;
  try {
    formatExternalGrantPermissionLevel({
      read: false,
      write: false,
      command: true,
    });
  } catch {
    rejected = true;
  }
  if (!rejected) throw new Error("command-only External grant was accepted");
});
