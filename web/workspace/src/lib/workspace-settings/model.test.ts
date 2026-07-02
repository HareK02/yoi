import {
  SETTINGS_PATTERNS,
  SETTINGS_PERMISSION_NOTICE,
  SETTINGS_ROUTE,
  SETTINGS_SECTIONS,
  settingsSectionHref,
} from "./model.ts";

declare const Deno: {
  test(name: string, fn: () => void): void;
};

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) {
    throw new Error(message);
  }
}

Deno.test("settings section navigation stays under the settings route", () => {
  assert(SETTINGS_ROUTE === "/settings", "settings route should be stable");

  for (const section of SETTINGS_SECTIONS) {
    const href = settingsSectionHref(section.id);
    assert(
      href.startsWith("/settings#"),
      `${section.id} should link under settings`,
    );
    assert(
      href.endsWith(section.id),
      `${section.id} href should preserve section id`,
    );
  }
});

Deno.test("settings shell advertises no fake browser admin model", () => {
  assert(
    SETTINGS_PERMISSION_NOTICE.includes("no browser user, role, permission"),
    "notice should explicitly deny a browser permission model",
  );
  assert(
    SETTINGS_PERMISSION_NOTICE.includes("does not create an admin role"),
    "notice should not imply an admin role exists",
  );
});

Deno.test("settings placeholders avoid mutation promises and raw authority leaks", () => {
  const allText = [
    SETTINGS_PERMISSION_NOTICE,
    ...SETTINGS_SECTIONS.flatMap((section) => [
      section.label,
      section.summary,
      ...section.bullets,
    ]),
    ...SETTINGS_PATTERNS.flatMap((pattern) => [pattern.title, pattern.body]),
  ].join("\n");

  assert(
    allText.includes(
      "does not add, remove, test, or persist Runtime endpoints",
    ),
    "Runtime Connections should remain a placeholder",
  );
  assert(
    allText.includes("Restart-required"),
    "restart-required pattern should be visible",
  );

  for (
    const forbidden of [
      "/home/",
      "socket path:",
      "token:",
      "secret:",
      "store root:",
    ]
  ) {
    assert(
      !allText.includes(forbidden),
      `settings copy should not expose ${forbidden}`,
    );
  }
});
