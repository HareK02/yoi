import {
  parseSkillCatalogResponse,
  parseSkillDetailResponse,
  SkillApiContractError,
} from "../src/lib/workspace/skills/api.ts";
import { SKILL_API_LIMITS } from "../src/lib/generated/skill-api.ts";

declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
};

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function assertEquals<T>(actual: T, expected: T): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

function assertContractError(value: () => unknown, expected: string): void {
  try {
    value();
  } catch (error) {
    assert(
      error instanceof SkillApiContractError,
      "expected SkillApiContractError",
    );
    assert(
      error.message.includes(expected),
      `expected bounded diagnostic containing ${expected}, got ${error.message}`,
    );
    assert(error.message.length <= 256, "diagnostic must remain bounded");
    return;
  }
  throw new Error("expected parser to reject malformed Skill response");
}

function builtinProvenance() {
  return {
    kind: "builtin",
    id: "builtin:errors",
    virtual_path: "skills/errors/SKILL.md",
    source_digest: "builtin-source-digest",
  };
}

function workspaceProvenance() {
  return {
    kind: "workspace",
    id: "workspace:release",
    virtual_path: "skills/release/SKILL.md",
    revision: 42,
    source_digest: "workspace-source-digest",
    tree_digest: "tree-digest",
  };
}

function catalogFixture(): Record<string, unknown> {
  return {
    authority: "workspace-config-skills-v1",
    projection: { config_revision: 42, tree_digest: "tree-digest" },
    entries: [{
      name: "errors",
      description: "Builtin guidance",
      activation_status: "active",
      projection_status: "valid",
      provenance: builtinProvenance(),
      overrides: [],
      diagnostics: [],
    }, {
      name: "release",
      description: "Workspace guidance",
      activation_status: "inactive",
      projection_status: "invalid",
      provenance: workspaceProvenance(),
      overrides: [builtinProvenance()],
      diagnostics: [{
        severity: "error",
        code: "invalid_projection",
        message: "invalid projected Skill",
        source: "workspace:release",
      }],
    }],
    diagnostics: [],
  };
}

function detailFixture(): Record<string, unknown> {
  return {
    authority: "workspace-config-skills-v1",
    projection: { config_revision: 42, tree_digest: "tree-digest" },
    name: "release",
    description: "Workspace guidance",
    provenance: workspaceProvenance(),
    overrides: [],
    diagnostics: [],
    activation_status: "active",
    projection_status: "valid",
    body: "# Release\n",
    allowed_tools: ["Bash"],
    allowed_tools_status: "experimental_hint_only",
    resources: [{
      kind: "reference",
      name: "skills/release/references/checklist.md",
      supported: true,
    }],
  };
}

Deno.test("Skill catalog parser accepts generated builtin, Workspace, and invalid projection shapes", () => {
  const parsed = parseSkillCatalogResponse(catalogFixture());
  assertEquals(parsed.entries.length, 2);
  assertEquals(parsed.entries[0].provenance.kind, "builtin");
  assertEquals(parsed.entries[1].activation_status, "inactive");
  assertEquals(parsed.entries[1].projection_status, "invalid");
  assertEquals(parsed.projection.config_revision, 42);
});

Deno.test("Skill detail parser preserves shared generated DTO fields", () => {
  const parsed = parseSkillDetailResponse(detailFixture());
  assertEquals(parsed.name, "release");
  assertEquals(parsed.allowed_tools, ["Bash"]);
  assertEquals(parsed.resources[0].supported, true);
});

Deno.test("Skill parser rejects stale Workspace projection revision and digest", () => {
  const staleRevision = catalogFixture();
  (staleRevision.projection as Record<string, unknown>).config_revision = 43;
  assertContractError(
    () => parseSkillCatalogResponse(staleRevision),
    "stale Workspace Skill projection",
  );

  const staleDigest = catalogFixture();
  (staleDigest.projection as Record<string, unknown>).tree_digest = "new-tree";
  assertContractError(
    () => parseSkillCatalogResponse(staleDigest),
    "stale Workspace Skill projection",
  );
});

Deno.test("Skill parser fails closed on unknown fields and newer enum values", () => {
  const unknownField = catalogFixture();
  unknownField.unexpected = true;
  assertContractError(
    () => parseSkillCatalogResponse(unknownField),
    "unknown fields",
  );

  const newerProvenance = catalogFixture();
  const entries = newerProvenance.entries as Record<string, unknown>[];
  (entries[0].provenance as Record<string, unknown>).kind = "remote_catalog";
  assertContractError(
    () => parseSkillCatalogResponse(newerProvenance),
    "unsupported Skill provenance kind",
  );

  const newerStatus = catalogFixture();
  const newerEntries = newerStatus.entries as Record<string, unknown>[];
  newerEntries[0].projection_status = "stale";
  assertContractError(
    () => parseSkillCatalogResponse(newerStatus),
    "unsupported Skill projection status",
  );
});

Deno.test("Skill parser rejects unsafe revisions and oversized collections or strings", () => {
  const unsafeRevision = catalogFixture();
  (unsafeRevision.projection as Record<string, unknown>).config_revision =
    Number.MAX_SAFE_INTEGER + 1;
  assertContractError(
    () => parseSkillCatalogResponse(unsafeRevision),
    "safe integer",
  );

  const oversizedCatalog = catalogFixture();
  const firstEntry = (oversizedCatalog.entries as unknown[])[0];
  oversizedCatalog.entries = Array.from(
    { length: SKILL_API_LIMITS.maxCatalogEntries + 1 },
    () => firstEntry,
  );
  assertContractError(
    () => parseSkillCatalogResponse(oversizedCatalog),
    "bounded array",
  );

  const oversizedDetail = detailFixture();
  oversizedDetail.body = "x".repeat(SKILL_API_LIMITS.maxBodyBytes + 1);
  assertContractError(
    () => parseSkillDetailResponse(oversizedDetail),
    "bounded string",
  );
});

Deno.test("Skill parser diagnostics never include rejected Skill body content", () => {
  const secret = "SENSITIVE-SKILL-BODY-CONTENT";
  const malformed = detailFixture();
  malformed.body = secret;
  malformed.provenance = {
    ...workspaceProvenance(),
    kind: "newer_source_kind",
  };
  try {
    parseSkillDetailResponse(malformed);
    throw new Error("expected malformed provenance to fail");
  } catch (error) {
    assert(
      error instanceof SkillApiContractError,
      "expected SkillApiContractError",
    );
    assert(
      !error.message.includes(secret),
      "diagnostic leaked Skill body content",
    );
    assert(error.message.length <= 256, "diagnostic must remain bounded");
  }
});
