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

  assert(
    settingsSectionHref("configuration-sources") === "/settings/configuration",
    "shared configuration editor route should stay canonical",
  );

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

Deno.test("settings shell advertises scoped account authority", () => {
  assert(
    SETTINGS_PERMISSION_NOTICE.includes("authenticated account authority"),
    "notice should identify authenticated account authority",
  );
  assert(
    SETTINGS_PERMISSION_NOTICE.includes("current Workspace owner"),
    "notice should state the Repository secret permission boundary",
  );
  assert(
    SETTINGS_PERMISSION_NOTICE.includes("does not expose secret material"),
    "notice should not imply that stored secret material is readable",
  );
});

Deno.test("Repository access settings are editable and canonically routed", () => {
  const section = SETTINGS_SECTIONS.find((entry) =>
    entry.id === "repository-access"
  );
  assert(
    section?.status === "editable",
    "Repository Access should be editable",
  );
  assert(
    settingsSectionHref("repository-access") === "/settings/repository-access",
    "Repository Access should have a dedicated settings route",
  );
  assert(
    section?.bullets.join("\n").includes("write-only"),
    "Repository Access copy should preserve write-only secret semantics",
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
