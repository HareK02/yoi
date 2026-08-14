import {
  diagnosticLabel,
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
      href.startsWith("/settings"),
      `${section.id} should link under settings`,
    );
    assert(
      !href.includes("#"),
      `${section.id} href should use a dedicated route instead of a page anchor`,
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

Deno.test("runtime connections are editable without advertising raw authority leaks", () => {
  const runtimeSection = SETTINGS_SECTIONS.find((section) =>
    section.id === "runtime-connections"
  );
  assert(
    runtimeSection?.status === "editable",
    "Runtime Connections should be editable",
  );

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
    allText.includes("restart_required=true") ||
      allText.includes("Restart-required"),
    "restart-required pattern should be visible",
  );
  assert(
    allText.includes("not echoed back") || allText.includes("not echoed"),
    "endpoint submission should not imply endpoint echoing",
  );

  for (
    const forbidden of [
      "/home/",
      "socket path:",
      "token:",
      "secret:",
      "store root:",
      "config file path:",
    ]
  ) {
    assert(
      !allText.includes(forbidden),
      `settings copy should not expose ${forbidden}`,
    );
  }
});

Deno.test("diagnostic labels preserve severity and code", () => {
  const diagnostic = {
    severity: "warning",
    code: "runtime_registry_restart_required",
    message: "Restart required.",
  } as const;
  assert(
    diagnosticLabel(diagnostic) ===
      "warning: runtime_registry_restart_required",
    "diagnostic label should be bounded and stable",
  );
});
