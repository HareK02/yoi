import type {
  CurrentSubjectPolicy,
  SubjectPermissionPolicyResponse,
} from "#lib/generated/memory-api.ts";
import type { ConfigurationSourceResponse } from "#lib/generated/configuration-adoption.generated.ts";
import type { PermissionResolution } from "#lib/generated/feature-permissions.generated.ts";

// Synthetic HTTP projections only. Backend resolver/persistence is not implemented by this fixture.
export function permissionSource(
  workspace = "w",
  subject = "s",
): ConfigurationSourceResponse {
  return {
    state: {
      snapshot: { digest: "sha256:source", entries: {} },
      contract: {
        contract_version: 1,
        decodal_version: "0.4.0",
        schema_version: 1,
        entrypoints: ["main.dcdl"],
        import_policy_version: 1,
        schema_bundle: {
          contributions: [],
          source: "builtin",
          fingerprint: "sha256:schema",
        },
        fingerprint: "sha256:contract",
      },
      projection_digest: "sha256:projection",
    },
    history_head: "head:adopted",
    source_stored: true,
    catalog_digest: "sha256:catalog",
    catalog: {
      workspace,
      namespaces: [{
        namespace: "ticket",
        feature: "builtin:ticket",
        enabled: true,
        declaration: {
          permissions: [
            {
              key: "ticket.hoge",
              description: "Explicit parent operation",
              children: ["ticket.hoge.fuga"],
            },
            {
              key: "ticket.hoge.fuga",
              description: "Explicit child operation",
              children: ["ticket.hoge.fuga.finish"],
            },
            {
              key: "ticket.hoge.fuga.finish",
              description: "Explicit leaf operation",
              children: [],
            },
            {
              key: "ticket.hoge.unlinked",
              description: "Same prefix, not included",
              children: [],
            },
          ],
          sets: [{
            set: "companion",
            children: [{
              kind: "namespace",
              value: { namespace: "ticket", keys: ["ticket.hoge"] },
            }],
          }],
        },
      }, {
        namespace:
          "plugin.long.namespace.with.a.deliberately.long.stable.identity.for.responsive.review",
        feature:
          "plugin:long-feature-identity-for-permission-origin-and-responsive-review",
        enabled: false,
        declaration: {
          permissions: [{
            key: "plugin.long.namespace.operation",
            description: "Disabled plugin contribution",
            children: [],
          }],
          sets: [],
        },
      }],
    },
    policies: [{
      subject_id: subject,
      change_id: "policy:one",
      execution: {
        grants: [{ kind: "set", value: "companion" }, {
          kind: "namespace",
          value: { namespace: "ticket", keys: ["ticket.hoge.fuga.finish"] },
        }],
        exclusions: [{
          kind: "namespace",
          value: { namespace: "ticket", keys: ["ticket.hoge.unlinked"] },
        }],
        revoked: false,
      },
      delegation: { grants: [], exclusions: [], revoked: true },
      execution_resolution: permissionResolution(workspace),
      delegation_resolution: permissionResolution(workspace, true),
      legacy_constraints: {
        change_id: "legacy:limited",
        execution: [{
          operation: "ticket.read",
          resource: {
            kind: "ticket",
            workspace_id: workspace,
            ticket_id: "T-837",
          },
        }],
        delegation: [],
      },
    }],
  };
}
// Dedicated current-policy projection. This fixture does not resolve authority.
export function permissionPolicyResponse(
  workspace = "w",
  subject = "s",
): SubjectPermissionPolicyResponse & { policy: CurrentSubjectPolicy } {
  const source = permissionSource(workspace, subject);
  const observed = source.policies[0];
  return {
    change_id: observed.change_id,
    policy: {
      change_id: observed.change_id,
      execution: observed.execution,
      delegation: observed.delegation,
      legacy_constraints: observed.legacy_constraints,
      reference_encoding: "feature_sets_v2",
    },
    can_manage: true,
    catalog: source.catalog,
    execution_resolution: observed.execution_resolution,
    delegation_resolution: observed.delegation_resolution,
  };
}

export function permissionResolution(
  workspace = "w",
  revoked = false,
): PermissionResolution {
  return {
    workspace,
    allowed: revoked ? {} : {
      "ticket.hoge.fuga.finish": [[
        { kind: "set", value: "companion" },
        { kind: "feature", value: "builtin:ticket" },
        { kind: "key", value: "ticket.hoge" },
        { kind: "key", value: "ticket.hoge.fuga" },
        { kind: "key", value: "ticket.hoge.fuga.finish" },
      ]],
    },
    diagnostics: revoked
      ? [{ kind: "revoked" }]
      : [{ kind: "excluded", value: "ticket.hoge.unlinked" }],
  };
}
